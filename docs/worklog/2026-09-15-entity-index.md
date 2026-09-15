# `e[:Transform]` が読めるようになった

2026-09-15（作業は 16 日未明まで）。`docs/plans/ecs-bridge-plan.md` 段階 A の到達点は
`tf = e[:Transform]` だったが、実装では `e.get(:Transform)` に落とした。理由は
`2026-09-15-ecs-bridge.md` の「だから読みは `e.get(:Transform)`」に書いたとおりで、VM の
`OP_GETIDX` が入れ子の実行ループでディスパッチしていて、その中ではタスクを待たせられなかったからである。
著者の判断で VM 側を本家と同じ形に直した（sabiruby の worktree `sabiruby-wt-idx`、ブランチ
`getidx-dispatch`、`docs/worklog/2026-09-15-getidx-dispatch.md`）ので、こちらは元の到達点に戻す作業。
worktree は `rubevy-wt-idx`、ブランチ `entity-index`。main には触っていない。

## VM の何が変わったか（こちらから見て）

mrbc は引数 1 個の `[]` を `OP_GETIDX` に畳む。sabiruby はその命令を `Vm::funcall` で回していた。
`funcall` は入れ子の実行ループで、積むフレームには `Cci::Skip`（ネイティブ境界の印）が付く。
`Task::Queue#pop` はブロックする前に `ci` を上から見て境界があれば断るので、`[]` の本体で
`Rubevy.ask(...).pop` を呼ぶと `blocking pop cannot be called from within a C function boundary`
になっていた。

変更後の sabiruby は本家と同じく、Array・Hash・String（で、かつその `[]` がまだ組み込みのまま）の
ときだけ命令が自分で答え、それ以外は**呼び出し元のフレームで `[]` を送る**。`Rubevy::Entity` は
`ObjKind::Data` なので当然その「それ以外」に入り、`Entity#[]` の本体は普通のフレームになる。
`method_missing` を直したとき（`2026-09-15-stage6c-proxy.md`）と同じ形の解決である。

手元の VM を使うために、rubevy の worktree の git 管理外の `.cargo/config.toml` に

```toml
[patch."https://github.com/sabiruby/sabiruby"]
sabiruby = { path = "../sabiruby-wt-idx" }
sabiruby-compiler = { path = "../sabiruby-wt-idx/compiler" }
```

を置いた（`.gitignore` にある。コミットしていない。`Cargo.lock` はコミット前に `git checkout --` で戻した）。

## 変えたもの

`src/prelude.rb` の `Rubevy::Entity` に `[]` を足した。中身は `get` を呼ぶだけで、`get` は残した。

```ruby
def [](name)
  get(name)
end
```

**`get` を消さなかった理由。** 1 つは互換（既に `get` で書いたスクリプトがある）。もう 1 つは、
これがホストへの往復で 1 フレーム掛かる読みであることが、`[]` という記号より `get` という語の方に
出るからである。`docs/host-api.md` は両方を載せ、`[]` を既定の書き方にした。

`tests/components.rs` の読みを `e[:Transform]` に、`examples/components.rs` が読む
`assets/scripts/components.rb` も同じに（`tools/compile_scripts.sh` で `.mrb` を作り直した）。
`get` のテストは 1 本残してある（`a_symbol_switches_an_enum_component` の
`Rubevy.entity.get(:Mode)`。同じ往復であることをそこで押さえる）。

文書は `docs/host-api.md` の "Components by name"（「なぜ `get` で `[]` ではないか」の段落を
「なぜ `[]` の中で待てるか」に書き換え）、`docs/plans/ecs-bridge-plan.md` の A の行、
`README.md`、`docs/outlook.md`、`docs/outlook.ja.md`、`docs/rust-bridge.ja.md`、`src/lib.rs` の
crate ドキュメント。過去の worklog（`2026-09-15-ecs-bridge.md`）は当時の記録なので直していない。

## 確かめたこと

**`e[:Transform]` が本当に待てること。** これが今回の全部なので、変更前の VM と並べて確かめた。
`.cargo/config.toml` の向き先を `../sabiruby`（main、`f691f5d`）にして `cargo test --test components` を
走らせると、`[]` で読む 4 本が落ちる:

```
failures:
    a_script_reads_a_transform_as_a_hash
    a_script_writes_one_field_and_leaves_the_others
    has_and_components_and_the_unregistered
    rubevys_own_questions_never_reach_the_game
test result: FAILED. 2 passed; 4 failed
```

落ち方は「スクリプトが `Rubevy.ask("read", ...)` まで辿り着かない」で、`[]` の中の `pop` が
境界で断られてタスクが死んでいる。向き先を `../sabiruby-wt-idx` に戻すと **6 本とも通る**。
落ちなかった 2 本（`a_symbol_switches_an_enum_component` と `find_answers_entity_objects`）は
読みに `get` を使っているものと `[]` を使っていないもので、ここも期待どおり。

**`cargo test --workspace`**: 全部 ok（components 6、events 3、host 8、futures 2、proxy 4、
replace 3、doc-tests 4）。

**examples**: `components`、`async`、`events`、`headless`、`sensor` を実行。`components` は
スクリプトが 4 回自分の `Transform` を読んで x を 1.0 ずつ動かし、ホストが最後に
`Vec3(5.0, 2.0, 3.0)` を見る（読みが待てているので 1 フレーム 1 歩で進む）:

```
[script] components: step 0, x = 1.0
...
[script] components: step 3, x = 4.0
host: the script left its Transform at Vec3(5.0, 2.0, 3.0)
```

**`cargo doc --no-deps`** 警告なし。**`cargo clippy --all-targets`** は **5 件で、変更前と同数**
（同じ tree を `git stash` して測り直した）。

## 気づいたこと

`Rubevy::Entity#[]` は Ruby の側に置いたままである（`define_fn` でネイティブにはできない）。
VM が `[]` を送るようになっても、ネイティブの中では `pop` で待てないことは変わらない
（sabiruby `docs/design/fibers.md` の Native boundaries）。待つのは Ruby、答えを作るのは Rust、
という `src/prelude.rb` の冒頭の理屈はそのまま生きている。

`[]` を Ruby で定義した以上、`e[:Transform]` は VM の早道には乗らず、毎回メソッド送信になる。
`Rubevy::Entity` は `ObjKind::Data` なので元からそうで、遅くなったところは無い。
そもそも 1 往復 1 フレームの読みなので、送信 1 回の差は見えない。
