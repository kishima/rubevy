# 段階 6c: 動的プロキシ（`proxy.rb`）— VM が直ってから

2026-09-15。sabiruby の `docs/plans/host-bridge-plan.md` 段階 6c の rubevy 側。前の担当が
`docs/worklog/2026-09-15-stage6bc-futures-proxy.md` で「今の VM では原理的に動かない」として止め、
著者が**案 A（VM の `method_missing` を `send` と同じ再ディスパッチにする）**を選んだ。
その VM 側の変更は同じ作業で sabiruby の `method-missing-dispatch` ブランチに入れてある
（sabiruby `docs/worklog/2026-09-15-stage6c-method-missing.md`）。worktree は `rubevy-wt-proxy`、
ブランチ `proxy`。

## 手元の VM に向ける

`.cargo/config.toml`（git 管理外。`.gitignore` の 2 行目に入っている）に

```toml
[patch."https://github.com/sabiruby/sabiruby"]
sabiruby = { path = "/home/kishima/book/kishima/sabiruby-wt-mm" }
sabiruby-compiler = { path = "/home/kishima/book/kishima/sabiruby-wt-mm/compiler" }
```

を置いた。`Cargo.toml` の依存のコメントが勧めている形そのままで、`sabiruby-compiler` の方も
同じ git URL から来ているので 2 行要る（片方だけだと crate が 2 つの別の VM を掴んで
`Vm` の型が合わなくなる）。`Cargo.lock` はこの patch で書き換わるので、コミットの前に
`git checkout -- Cargo.lock` で戻した。

## `proxy.rb`

前の担当が worklog に残した下書きをほぼそのまま使った。足したのは 3 つ:

* `attr_reader :kind`。`method_missing` が何でも受けてしまうので、プロキシ自身に聞きたいこと
  （どの相手の代理か）が質問として外に出て行ってしまわないよう、本物のメソッドにしておく。
* `&blk` を仮引数に取る。今は捨てているが、`Rubevy.ask` にブロックを渡す先が無いだけで、
  呼び出し側が `robot.each { }` と書いたときに `ArgumentError` ではなく素通りするほうが、
  「代理」としての振る舞いとして一貫している。
* `alias to_s inspect`。`"#{robot}"` が `#<Rubevy::Proxy robot>` になる。これが無いと
  `to_s` が `method_missing` に落ちて、文字列に埋めただけで質問が 1 つ飛ぶ。

`respond_to_missing?` が常に true なのは下書きのまま。プロキシは何にでも答える（答えるのは
ゲームだが、Ruby 側から見れば呼べば呼べる）ので、`respond_to?` と実際の呼び出しが食い違わない。

`.mrb` は Docker の本家 `mrbc`（`kishima/mruby:4.1.0-rc`、`tools/compile_scripts.sh` と同じイメージと
同じ呼び方）で作った。念のため手元の VM の `sabiruby compile` でも作って `cmp` したところ
**577 バイトが 1 バイトも違わなかった**ので、どちらで作っても同じだと確かめられた
（`async.mrb` も同じく一致）。Docker が作るファイルは root 所有になるので、同一と確かめたうえで
`sabiruby compile` の方を置いてある。

## 例

`assets/scripts/async.rb` に `require "proxy"` と

```ruby
robot = Rubevy::Proxy.new("robot")
Rubevy.log "async: #{robot.inspect} is at #{robot.move_to(1, 2).inspect} after moving, at frame #{$rubevy[:frame]}"
```

を足し、`examples/async.rs` に `"robot.move_to"` の腕を足した（`Answer::List(vec![x, y])` で答える）。
前の担当が `blocking pop cannot be called from within a C function boundary` で落ちたのと
**同じフレーム 7 の同じ行**が、今は通る:

```
host: frame 1: a path to (8, 3) — handing it to the task pool
[script] ticker: frame 3, shared=1
[script] ticker: frame 5, shared=1
[script] async: the path arrived at frame 6: 8 steps, ending at [8.0, 3.0]
host: frame 7: the robot is moving to (1, 2)
[script] async: #<Rubevy::Proxy robot> is at [1.0, 2.0] after moving, at frame 7
host: script on 33v0 ended: Finished :async_done
```

`Rubevy.log` の 1 行に `robot.inspect` と `robot.move_to(1, 2)` が両方入っているのは意図してのこと。
`inspect` は本物のメソッドなのでホストに何も届かず、`move_to` だけが質問になる — ホストのログに
`robot.move_to` が 1 回しか出ていないのが、その証拠になっている。

## テスト（`tests/proxy.rs`、4 本）

`tests/futures.rs` の作り（`MinimalPlugins` + `run_once`、`Seen` リソースに届いたものを溜める、
`frames(&mut app, n)`）をそのまま借りた。スクリプトは `require "proxy"` で
**`assets/scripts/proxy.mrb` を本当に読む** — テストの中に `proxy.rb` を書き写すと、
出荷するファイルが壊れていても通ってしまうため。`RubevyPlugin::default()` の load path が
`assets/scripts` なので、テストは crate の root から走れば何も足さずに届く。

1. `a_call_on_a_proxy_reaches_the_game_as_kind_dot_name`: `robot.move_to(1, 2)` と `robot.hp` が
   `("robot.move_to", [1.0, 2.0])` と `("robot.hp", [])` としてこの順で届く。
2. `the_answer_is_the_value_of_the_call`: ホストが 10 と 20 で答え、スクリプトが `"10.0/20.0"` を
   送り返す。止まれなければどちらの値も見られないので、これが「答えが返る」の確認になる。
3. `a_proxy_answers_respond_to_and_says_what_it_stands_for`: `respond_to?` が 2 つとも true、
   `inspect` と `kind` は質問にならず `#<Rubevy::Proxy robot> robot` として返る。
4. `one_proxy_parks_its_task_while_another_script_runs`: `4.times { robot.step }` の隣で別の
   スクリプトが回り続ける。質問はちょうど 4 回（1 回も落ちていない）で、隣は 4 回より多く回る。

`cargo test --workspace` は 13 本すべて通る（entity 3、futures 2、proxy 4、replace 3、doctest 1）。
`cargo run --example async` は上の出力で最後まで走る。`cargo doc --no-deps` 警告なし。
`cargo clippy --all-targets` の警告は 2 件で、作業前と同じ（`drain_commands` の `collapsible_if` と
`examples/headless.rs` の `type_complexity`）。

## 文書に書いたこと

`docs/host-api.md` に「A dynamic proxy (`Rubevy::Proxy`)」の節を、`answer_with` の次に置いた。
書いたのは、使い方、**明示登録が基本で、外部オブジェクトにだけ使う**という設計議論の結論と
その理由（本物のメソッドは引数を言い、名前の間違いが呼び出しで分かり、探索も安い。プロキシは
何も言わないので、間違いは「ゲームが知らない質問」になって実行時まで分からない）、そして
これが VM の `method_missing` の再ディスパッチに依っていること。README の箇条書きと例の行、
`docs/README.md` の 2 つの表も直した。

## 迷ったところ

`Rubevy::Proxy` を `assets/scripts/` の Ruby に置くか、rubevy が VM の起動時に Rust から
`define_class` で入れるかを考えた。計画書が「`require` できる Ruby のライブラリ」と言っているので
Ruby のままにしたが、Rust 側に置くと `require` が要らず、`ruby-source` feature の有無にも
左右されない（`.mrb` なので今も左右されないが）。一方で、Ruby のファイルなら
ゲームが読んで直せるし、プロキシの作りは「ゲームごとに変えたくなる部分」（名前の付け方、
`respond_to_missing?` を厳しくする、答えをキャッシュする）なので、見える場所にある方がよい。
`assets/scripts/` に置いた理由はそれで、決め直す材料ではないと判断した。
