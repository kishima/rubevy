# 作業記録: ホスト状態への移行とエンティティの Data 化

rubevy 側で `docs/rust-bridge.ja.md` 6 章「まだ滑らかでないところ」の 2 項目（`static` のコマンドキュー、
エンティティ番号を数値で渡していること）を、SabiRuby の段階 3〜5 で入った API
（`define_closure` / `set_host_state` / `data_new` / `set_on_free`）で直したときの記録。
結果だけでなく、読んだ場所、確かめた入力と出力、捨てた案を残す（同人誌の「Rust でホストとつなぐ」章の素材）。

作業は worktree `rubevy-wt-host`、ブランチ `host-state`。ベンチは回していない（別の担当が同じ機械で
sabiruby の計測中のため）。ゲーム（rubevy_games）には触っていない。

## 0. 手をつける前に読んだところ

- rubevy `src/lib.rs:371-386` — `static COMMANDS: Mutex<Vec<HostCommand>>` と `push_command` / `take_commands`。
  ここが今回の 1 つ目の対象。
- `src/lib.rs:604-668` — `install_host_api`。`Rubevy.log` / `.spawn` / `.despawn` / `.entity` / `.move_to` /
  `.ask` / `.set_position` の 7 本。すべて `define_method`（`fn` ポインタ）。
- `src/lib.rs:477`（`start_scripts`）と `:671-675`（`current_entity`）— エンティティ番号は `Entity::to_bits` を
  `Value::Int` にしてタスクの `@rubevy_entity` に置き、ネイティブは「いま走っているタスク」から読む。
- `src/lib.rs:327-353` — `ScriptWorld::answer` の `Answer` → `Value` 変換。`Rows` / `List` は `Value::Float`。
  ゲーム（`rubevy_games/sabibots/src/main.rs:1239`）は `Answer::Num(e.to_bits() as f64)` でエンティティを返しており、
  ここは互換のため数値のまま残す必要がある。
- sabiruby `src/vm.rs:1082-1120`（`define_closure`）、`:1124-1158`（`set_host_state` / `host_state` /
  `host_state_mut` / `take_host_state`）、`:1160-1220`（`data_new` / `data_of` / `set_on_free`）。
- sabiruby `src/convert.rs:1-60`（`define_fn` の形）、`:176-220`（`DataRef` と `This<T>`）。
- sabiruby `tests/data.rs` — Data は既定で `==` / `eql?` / `hash` が `(tag, handle)` 一致、`inspect` は
  `#<Player:0x…>`、`dup` / `clone` は `TypeError`。**`==` と `inspect` はホストが何もしなくても動く**ので、
  足すのは `to_i` だけでよい、と分かった。
- sabiruby `docs/host-bridge-plan.md` の「実装で分かったこと」段階 3・4・5。解放フックは sweep の途中ではなく
  `gc_collect` の末尾で呼ばれ、`&mut Vm` は渡らない。

## 1. `static COMMANDS` をなくす

### 1.1 なぜ static だったか

ネイティブの型は `fn(&mut Vm, Value, &[Value], Value) -> VmResult<Value>` という**関数ポインタ**で、環境を
持てない。だから「ネイティブが書いてシステムが読む」キューの置き場所が、プロセスに 1 つの `static` しか
なかった。`&mut Vm` は来るので VM の中には置けたはずだが、VM 側に「ホストが預けた任意の値」の受け口が
無かった。段階 3 がその両方（`define_closure` と `host_state`）を足した。

### 1.2 何をどう置き換えたか

キューは `HostState { commands: Vec<HostCommand> }` という私的な構造体にして、`ScriptWorld::new` で
`vm.set_host_state(HostState::default())` する。`push_command` / `take_commands` は `&mut Vm` を取り、
`vm.host_state_mut::<HostState>()` で降ろす形にした。呼び出し側の変更は、ネイティブの中では
`push_command(vm, HostCommand::…)`、`drain_commands` では `take_commands(&mut world.vm)` の 2 か所だけ。

`install_host_api` の 7 本は `define_closure` に変えた。この段階では 7 本とも環境を捕まえていないので
`define_method` のままでも動くが、段階 2（エンティティの Data 化）で `Rubevy::Entity` クラスの `ObjId` を
ネイティブが必要とするため、そこで環境が要る。1 か所だけクロージャにして残りを関数ポインタにする形は、
読む側が「なぜこの 1 本だけ違うのか」を考えることになるので採らなかった。

`Mutex` が消えたので `lock()` の失敗（毒された Mutex）を握り潰していた `if let Ok(..)` も消えた。
`pending_command_count()` は引数無しの関数だったが、VM の中を見るので `&ScriptWorld` を取る形に変えた
（`#[doc(hidden)]`、テスト用で、リポジトリ内にも rubevy_games にも呼び出しは無い）。

### 1.3 テストを 3 本に戻す — 壊れ方を先に確かめた

`tests/replace.rs` は「キューが static なので、並行に走る 2 つの `App` が互いの質問を取り合う」という理由で
3 つのテストを 1 本の `#[test]` にまとめてあった。分けたうえで、**まず古い実装（`git checkout HEAD -- src/lib.rs`）に
新しいテストを当てて、本当に壊れることを確かめた**:

```
running 3 tests
test a_despawned_script_stops ... FAILED
thread 'a_despawned_script_stops' panicked at sabiruby/src/object.rs:369:
access to freed object ObjId(667)
```

予想していた壊れ方は「質問の数が合わない」だったが、実際にはもっと悪かった。`Request` は
`ObjId`（VM ヒープの添字）を持つので、A の VM が作ったキューの `ObjId` が B の VM の
`task_queue_push` に渡り、B のヒープの無関係なスロット（このときは解放済み）を指した。
ObjId は VM ごとの添字で、VM をまたぐと意味を持たない。static を使うということは、
その「VM をまたがない」という約束を型で守れないということでもあった。

新しい実装では同じ 3 本が並行に通る:

```
running 3 tests
test a_script_stuck_under_a_native_does_not_hold_the_frame ... ok
test a_despawned_script_stops ... ok
test a_replaced_script_stops ... ok
test result: ok. 3 passed
```
