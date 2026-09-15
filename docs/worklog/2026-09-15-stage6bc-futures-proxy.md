# 段階 6b・6c: Future で答える（`answer_with`）と動的プロキシ（`proxy.rb`）

2026-09-15。sabiruby の `docs/plans/host-bridge-plan.md` 段階 6b・6c を rubevy 側で実装した記録。
worktree は `rubevy-wt-async`、ブランチ `futures-proxy`。main には触っていない。sabiruby 側は別の担当が
段階 6a（マクロ crate）を進めているので、この作業では sabiruby のコードには一切触れていない
（VM は `Cargo.toml` の git 依存 `b3cd818` のまま）。

## 読んだところ

出発点は `src/lib.rs` の `Rubevy.ask` の往復である。スクリプトが `Rubevy.ask("scan", 40)` を呼ぶと、
ネイティブ（`install_host_api` の `ask`、`src/lib.rs:760` 付近）が mruby-task のキューを 1 本作って
`gc_register` し、`HostCommand::Ask { entity, kind, args, queue }` を VM のホスト状態の待ち行列に積んで、
キューそのものを返り値にする。スクリプトは `.pop` でそのキューに乗って止まる。フレームの終わりに
`drain_commands`（`src/lib.rs:623`）がコマンドを取り出し、`Ask` だけは `ScriptWorld::requests` に移す。
ゲームは `take_requests`（`src/lib.rs:366`）で受け取り、好きなフレームで `answer`（`src/lib.rs:379`）を呼ぶ。
`answer` は `Answer` を `Value` に直して `task_queue_push` し、`gc_unregister` でキューを手放す。

つまり「答えを後で返す」仕掛けは**すでに全部ある**。段階 6b が足すのは、その「後で」を
ゲームの system が自分で覚えておく代わりに、Bevy のタスクプールに預けられるようにすることだけである。
`Request` は `Clone` な素の構造体で、`queue: ObjId` を持っているだけなので、フレームをまたいで持ち越しても
何も壊れない（`gc_register` 済みなので回収もされない）。ここが確かめたかった前提で、確かめられた。

## 6b: `ScriptWorld::answer_with`

実装は `ScriptWorld` に `answering: Vec<(Request, Task<Answer>)>` を 1 本持たせるだけになった。
`answer_with(request, fut)` が `AsyncComputeTaskPool::get().spawn(fut)` して組にして積み、毎フレームの
system が `block_on(poll_once(&mut task))` で完了したものを拾って既存の `answer` に渡す。
`Answer` を作る側も渡す側も変わらないので、スクリプトからは今までの `ask(...).pop` と区別がつかない。
公開 API の境界は `impl Future<Output = Answer> + Send + 'static` にした（後述のとおり、これで
`multi_threaded` あり・なしの両方の `TaskPool::spawn` を満たす）。

### system をどこに置くか

計画書は「`drain_commands` の隣」としか言っていないので、`tick_scripts` の前と後を比べた。

後（`drain_commands` の次）に置くと、フレーム N の途中で完了した future はフレーム N の終わりに
`answer` され、待っているスクリプトが動くのは次のフレーム N+1 の `tick_scripts` になる。
前（`start_scripts` の次、`tick_scripts` の前）に置くと、**前のフレームの終わりから今フレームの頭までの間に**
完了した future は、今フレームの `tick_scripts` でそのまま拾われる。future が完了する時刻はフレームの
どこでもありえて、描画のあるアプリではフレームの大半は Update の外（表示待ち）なので、前に置いたほうが
平均で 1 フレーム早い。遅くなる場合は無い（今フレームの Update 中に完了したものは、どちらに置いても
次フレームの `tick_scripts` で拾われる）。よって `(start_scripts, deliver_answers, tick_scripts, drain_commands)`
の順にした。`deliver_answers` が触るのは `ScriptWorld` だけで、`tick_scripts` の前に VM に書き込むのは
`answer`（キューへの push）だけなので、スケジューラを動かしている最中の再入も無い。

## Bevy の `multi_threaded` を確かめた

`bevy_tasks` の `multi_threaded` が**無いとどうなるか**を、依存の実物（`~/.cargo/registry/.../bevy_tasks-0.19.1`）で
確かめた。`cargo tree -e features -i bevy_tasks` では、rubevy の今の指定（`default-features = false`,
`bevy_asset` と `bevy_log`）で有効なのは `async_executor` と `futures-lite` だけで、`multi_threaded` は**付いていなかった**。
その場合 `src/single_threaded_task_pool.rs` が使われ、`TaskPool::spawn` はこうなっている（190 行付近）:

```rust
LOCAL_EXECUTOR.with(|executor| {
    let task = executor.spawn(future);
    while executor.try_tick() {}   // 進められるところまで、その場で回す
    task
})
```

つまり**メインスレッドで、進めるだけ進めてから帰ってくる**。計算するだけの future（`yield_now` で
譲るものも含む。譲ると再スケジュールされ `try_tick` が真を返し続けるので）は `answer_with` を呼んだ
その場で最後まで走り、フレームを止める。外からの合図を待つ future（waker を登録して Pending を返すもの）は
そこで止まり、合図のあとに `TaskPoolPlugin` が `Last` に入れている `tick_global_task_pools` が進めてくれるので、
1 スレッドでも正しく動く。`AsyncComputeTaskPool::get()` はプールが未初期化だと panic するが、
`MinimalPlugins` にも `DefaultPlugins` にも `TaskPoolPlugin` が入っている（`bevy_internal/src/default_plugins.rs:164`）ので、
普通の App なら問題にならない。

結論: **rubevy 自身は `multi_threaded` を要求しない**（lib は素のままビルドできる）。本当に別スレッドで
動かしたい利用者が、自分の `bevy` 依存に `multi_threaded` を足す。rubevy の例とテストは別スレッドで
動いてほしいので、dev-dependency の `bevy` にだけ `multi_threaded` を足した。README と `docs/host-api.md` に
この違いを書いた。

## 6b の例とテスト

`examples/async.rs` は `headless` と同じ MinimalPlugins に、8 回に分けて 10 ms ずつ寝る「経路探索」を足したもの。
実行すると質問がフレーム 0、答えがフレーム 6 で、その間ティッカーはフレーム 3・5 で動いている:

```
[script] async: asking for a path to (8, 3) at frame 0
host: frame 1: a path to (8, 3) — handing it to the task pool
[script] ticker: frame 3, shared=1
[script] ticker: frame 5, shared=1
[script] async: the path arrived at frame 6: 8 steps, ending at [8.0, 3.0]
```

`tests/futures.rs` では最初 `std::thread::sleep` の future で「まだ来ていない」を確かめようとしたが、
それは機械の速さとの競争になる（遅い機械では 10 フレームのうちに終わってしまうかもしれない）ので捨てた。
代わりにテストが自分で開ける門（`bool` と `Waker` を持つ 1 回限りの小さな oneshot、`Gate`）を作り、
future はそれを待つ。門が閉じている間は、待っているスクリプトの質問はちょうど 1 回（＝ `pop` で止まっている）で、
隣のスクリプトは何周もしている。門を開けると待っていた方が動き出す。この形なら `multi_threaded` の無いビルドでも
同じ結果になる。2 本目は future が作った値（`Answer::Num(7.0)`）がそのまま `pop` の返り値になることを、
スクリプトが受け取った値を `Rubevy.ask('got', v)` でホストに送り返して確かめている。

## 6c: 動的プロキシ — **計画の前提が VM と合わなかったので、ここで止めた**

`assets/scripts/proxy.rb` を書き、`.mrb` に落とし、`examples/async.rs` の例に
`robot = Rubevy::Proxy.new("robot"); robot.move_to(1, 2)` を足して動かしたところ、スクリプトがこう落ちた:

```
host: frame 7: the robot is moving to (1, 2)
host: script on 33v0 ended: Failed #<RuntimeError: blocking pop cannot be called from within a C function boundary>
```

ホストは質問（`"robot.move_to"`）をちゃんと受け取って答えている。落ちているのは**スクリプトが `pop` で止まろうとした瞬間**である。

原因は VM 側にある。`sabiruby/src/builtins/ext_task.rs:886` の `queue_pop_try` は、キューが空で待つ必要があるとき
`vm.fiber_check_native(vm.cur)` を見て、呼び出し元のフレームのどこかに native の境界（`Cci::Skip`）があれば
`RuntimeError` にする。そして `sabiruby/src/vm.rs:3839` の method_missing の分岐は、見つけた Ruby の `method_missing` を
`call_proc_with` で**入れ子の実行ループとして**呼んでいる（`send` のための `op_send_redirect` のような
「今のフレームに積み直す」経路は method_missing には無い）。つまり `method_missing` の中は常に native の境界の内側で、
そこからタスクを切り替えることはできない。

切り分けのために rubevy の中で 6 通りを実際に走らせた（`tests/tmp_probe.rs`、確認後に消した。
いずれもホストが `Answer::Num(7.0)` で答える同じ構え）:

| 書き方 | 結果 |
|---|---|
| `def method_missing(n, *a); Rubevy.ask("p.#{n}", *a).pop; end` | **RuntimeError**（C boundary） |
| `define_method(:foo) { \|x\| Rubevy.ask('d.foo', x).pop }` | 通る |
| `def foo(x); Rubevy.ask('n.foo', x).pop; end` | 通る |
| `def method_missing(n, *a); Rubevy.ask("q.#{n}", *a); end` → 呼び出し側で `.pop` | 通る |
| `def initialize(k); @n = Rubevy.ask('i.names', k).pop; end` | **RuntimeError**（`Class#new` が native なので同じ理由） |
| `initialize` の中で特異クラスに `define_method` し、あとから呼ぶ | 通る |

分かることは 2 つ。(1) **止まれるのは「バイトコードから普通に呼ばれた Ruby のメソッド」の中だけ**で、
`method_missing` も `initialize` も native 経由なので止まれない。(2) `define_method` で作ったメソッドは
普通のメソッドなので止まれる。`pop` はキューに既に値があるときは境界を見ずに返す（`ext_task.rs:865`）が、
`ask` の答えが同じフレームの `drain_commands` より前に入っていることはありえないので、この抜け道は使えない。

計画書 6c の形（`method_missing` の中で `ask(...).pop`）は、**今の VM では原理的に動かない**。
著者の判断が要るので実装は止めた。`examples/async.rs` と `assets/scripts/async.rb` は 6b だけの形に戻してあり、
`proxy.rb` は入れていない（下書きは下に残す）。案は 3 つ:

* **案 A（VM を直す。計画の形がそのまま通る）**: sabiruby の `op_send` の method_missing 分岐を、
  `send` の `op_send_redirect` と同じ「今のフレームで再ディスパッチする」形にする。そうすれば
  `method_missing` の中は境界ではなくなり、`Rubevy.ask(...).pop` も `Fiber.yield` も `break` も効く。
  本家 mruby も `mrb_funcall` ではなく `mrb_exec_irep` で method_missing に入るので、振る舞いとしても本家寄りになる。
  ただし sabiruby 側の変更で、いま別の担当が段階 6a を進めている。rubevy の `proxy.rb` はこの 1 行の変更を待つだけでよい。
* **案 B（Ruby 側だけで済ませる。形が少し変わる）**: `method_missing` は `.pop` せずキューを返す。
  スクリプトは `robot.move_to(1, 2).pop` と書く。`Rubevy.ask(...)` が返すものと同じなので一貫はしているが、
  「呼び出しが呼び出しに見える」という 6c の売りは半分になる。
* **案 C（明示登録に寄せる）**: `Rubevy::Proxy.new("robot", [:move_to, :hp])` が `define_method` で本物のメソッドを作る。
  呼び出しは普通のメソッド呼び出しなので止まれて、形も `robot.move_to(1, 2)` のまま。名前の一覧が要るぶん
  「動的」ではなくなるが、設計議論の結論（明示登録が基本）には最も近い。名前の一覧をホストに聞くことはできない
  （`initialize` の中では止まれないため）ので、一覧はスクリプトが書くか、別の仕掛けで渡すことになる。

下書きの `proxy.rb`（案 A が通れば、そのまま使える形）:

```ruby
module Rubevy
  class Proxy
    def initialize(kind)
      @kind = kind
    end

    def method_missing(name, *args)
      Rubevy.ask("#{@kind}.#{name}", *args).pop
    end

    # A proxy answers for everything, so that `respond_to?` agrees with what a call does.
    def respond_to_missing?(name, include_private = false)
      true
    end

    def inspect
      "#<Rubevy::Proxy #{@kind}>"
    end
  end
end
```

`tools/compile_scripts.sh` は `assets/scripts/*.rb` を全部回すので、`proxy.rb` を置けば対象に入る（変更不要）。
今回 `async.mrb` を作るのにも使った（Docker はすでに動いていたのでそのまま利用。起動も再起動もしていない）。
`method_missing` そのものと `respond_to_missing?` は VM で問題なく動くことを CLI で確かめてある
（`sabiruby run` で `p r.move_to(1, 2)` → `["ask", "robot.move_to", [1, 2]]`、`r.respond_to?(:anything)` → `true`）。
動かないのは「その中で止まること」だけである。

## 確認したこと

* `cargo test --workspace`: 8 passed（`entity` 3、`replace` 3、`futures` 2）+ doctest 1。
* `cargo build --examples`、`cargo run --example async` は最後まで動く（上の出力）。
* `cargo doc --no-deps`: 警告なし。
* `cargo clippy --all-targets`: 警告 2 件で、作業前と同じ（`drain_commands` の `collapsible_if` と
  `examples/headless.rs` の `type_complexity`。どちらも既存）。
