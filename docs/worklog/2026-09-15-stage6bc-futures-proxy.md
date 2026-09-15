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
