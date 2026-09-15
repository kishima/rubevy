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

## 2. エンティティを `Data` で渡す

### 2.1 何が壊れていたか

エンティティは 2 つの経路で Ruby に渡っていた。`Rubevy.entity` は `Value::Int`（`Entity::to_bits` は 64 ビット、
`Int` は `i64` なので正確）、ゲームの答えに載せる場合は `Answer::Num` / `List` / `Rows` の `f64`。
後者が問題で、`f64` が正確なのは 2^53 まで。Bevy の `Entity::to_bits` は世代を上位 32 ビットに置くので、
世代が 2^21 を超えると下位が落ちる。実用上まず起きないが、「型としては正しくない」（6 章）。

正確さとは別に、Integer で渡すことには「型が無い」問題もある。`Rubevy.despawn 3` と書けてしまい、
3 は誰かのエンティティになる。数値の演算もできる（`entity + 1`）。

### 2.2 クラスをどう作るか — `define_class_under` が無い

Data オブジェクトには入れ物のクラスが要る。`Rubevy::Entity` にしたいが、VM の `define_class(name, super)` は
**Object の下にしか置けない**（`src/vm.rs:1058`。定数を `self.core.object` の `consts` に入れる）。
`define_class_under` に当たるものは無い。3 案を比べた:

1. `vm.define_class("Entity", object)` でトップレベルの `Entity` にする。ゲーム側が `Entity` という名前を
   使いたい場合に衝突するし、`Rubevy` にぶら下がっていないので由来が読めない。
2. `vm.heap.class_mut(m).consts.insert(..)` で自分でぶら下げる。まさに今回減らしたい「VM の内部に直接触る」形。
3. Ruby がやるとおりにする: `Class.new(Object)` を `funcall` で作り、`Rubevy.const_set(:Entity, it)` で名前を付ける。

3 を採った。`const_set` の実装（sabiruby `src/builtins/object.rs:136`）は、渡されたクラスが無名なら
`name` と `outer` を埋めるので、これだけで `Rubevy::Entity` という完全な名前になる（`e.class.to_s` が
`"Rubevy::Entity"` を返すことをテストで確かめた）。公開 API だけで済み、VM 側に新しい入口を足す必要もない。

### 2.3 Ruby から見える形

Data オブジェクトは VM 側で既に `==` / `eql?` / `hash` が `(tag, handle)` 一致、`equal?` は同一性、
`dup` / `clone` は `TypeError`、`inspect` は `#<Rubevy::Entity:0x…>` になる（sabiruby `tests/data.rs`）。
ホストが足したのは `to_i`（ハンドル = bits）と `inspect` / `to_s` の 3 本だけで、いずれも `define_fn` と
`This<DataRef>` で 1 行:

```rust
vm.define_fn(entity, "to_i", |this: This<DataRef>| this.handle as i64);
```

`inspect` は既定のままでも「使える」が、`p Rubevy.entity` が `#<Rubevy::Entity:0x000000000040>` としか
言わないのはデバッグに役立たないので、Bevy の `Entity` の `Display` を借りて `#<Rubevy::Entity 3v1#…>` の形にした。
`to_s` も同じにしてある（`"#{e}"` が `#<Rubevy::Entity>` になるのを避けるため）。

### 2.4 タスクのインスタンス変数は Int のまま — オブジェクトは呼ぶたびに作る

`Rubevy.entity` が返すオブジェクトを、タスクの `@rubevy_entity` に持たせて使い回す案と、呼ばれるたびに
新しく作る案があった。後者にした理由は 3 つ:

* `move_to` と `ask` は「いま走っているタスクのエンティティ」を **`u64` として**使う。ivar が Int のままなら
  そのまま読めるが、Data を入れると毎回 `data_of` で開けることになる。
* 同じエンティティの 2 つのオブジェクトは `==` でも `hash` でも等しい（VM 側がハンドルで比べるため）ので、
  スクリプトからは区別が付かない。区別が付くのは `equal?` だけで、これを使うスクリプトは想定していない。
* 捨てられたオブジェクトができるので、**解放フックが呼ばれることをテストで観察できる**（2.6）。

代償は呼び出しごとに 1 オブジェクトの割り当て。ループの中で `Rubevy.entity` を何度も呼ぶスクリプトは
ごみを作るが、GC はスケジューラの idle 点で回る設定なので、フレームの途中で止まることはない。
気になるスクリプトは `e = Rubevy.entity` と一度受ければよい。

### 2.5 `Answer` と `Arg` — ゲームを壊さない範囲

`Answer::Entity(Entity)` と `Arg::Entity(Entity)` を**足した**。`List` と `Rows` は数値のまま残してある。
理由は互換性で、SabiRuby Battle は `Answer::Num(e.to_bits() as f64)`（`rubevy_games/sabibots/src/main.rs:1239`）で
エンティティを返し、`radar` は 1 台 1 行の数値の表を返す。表の中の 1 セルだけオブジェクトにする形は
`Rows(Vec<Vec<f64>>)` の型に入らないし、表は数値の表であることに意味がある。
`Request::num` / `text` の振る舞いも変えていない: エンティティの引数に対しては `num` も `text` も `None` を返す
（`as_num` / `as_text` の `match` に variant を足しただけ）。読むための入口は `Arg::as_entity` と
`Request::entity_arg(i)` を新設した。

`Rubevy.despawn` と `Rubevy.set_position` は `entity_arg()` という小さな関数で「Data なら handle、
そうでなければ `expect_int`」を見る。Integer を受ける経路を残したのは、既存のスクリプトと、
ゲームが `Answer::Num` で返したエンティティ（上の互換）を受け取ったスクリプトのため。

### 2.6 解放フック — 何も持たないものに登録する意味

エンティティの Data はホスト側の資源を持たない。ハンドルが `Entity::to_bits` そのもので、
そのエンティティが生きているかを知っているのは ECS だからだ。だからフックの中ですることは無い。
それでも `set_on_free` を登録したのは、(1) ハンドルを slab に持つ種類の Data を後で足すときの置き場所を
決めておくため、(2) 経路が本当に動くことを一度確かめるため。

フックは `&mut Vm` をもらえない（GC の終わりに走るため）ので、観察できるのは外に持ち出した値だけになる。
`Arc<AtomicU64>` を 1 つ捕まえて数えるだけにし、`ScriptWorld::freed_entities()` で読めるようにした
（`Vm` が `Send + Sync` であることを壊さない形でもある）。テストは 300 個作って `GC.start` し、
250 個以上が回収されたことを見る。

```
running 3 tests
test a_script_holds_its_entity_as_an_object ... ok
test an_entity_the_game_answered_with_comes_back_the_same ... ok
test the_host_is_told_when_an_entity_object_is_collected ... ok
```

### 2.7 VM の内部フィールドに直接触っている残り

減らせたのは `install_host_api` の周りだけで、次の 3 つは残した。いずれも**公開 API に相当するものが
VM 側に無い**ためで、rubevy 側で直せるものではない（sabiruby に入口を足す仕事になる）。

* `vm.heap.ivar_set` / `ivar_get`（`start_scripts` と `current_entity`）— タスクにエンティティ番号を持たせる。
  `Vm` にインスタンス変数の入口は無い（`object.rs` の `Heap` のメソッドとしてのみ公開）。
  `funcall` で `instance_variable_set` を呼ぶ案は、毎フレームではないとはいえ、
  Ruby のメソッド呼び出し 1 回を挟むのと、`@rubevy_entity` という名前を Ruby から書き換えられる点で同じなので採らなかった。
* `vm.task.running`（`current_entity`）— いま走っているタスク。`task_*` の公開関数は 20 本あるが、
  「走っているのはどれか」を答えるものが無い。
* `vm.globals`（`set_frame_state`）— `$rubevy` を毎フレーム置く。グローバル変数の公開入口も無い。
* `vm.heap.get(o).kind`（`is_exception`）— 終わったタスクの結果が例外かどうかを見る。`class_of` はあるが
  「Exception の子孫か」を答える公開関数は無く、`funcall` で `is_a?` を呼ぶと Ruby のメソッド呼び出しになる
  （タスクが終わったフレームだけとはいえ、判定のために VM を動かすことになる）ので、そのままにした。

この 4 つは「`Vm` の公開フィールド」なので `unsafe` でも回避策でもないが、VM の内部構造に対する依存ではある。
`grep -rn unsafe src/ tests/ examples/` は 0 件のまま。

## 3. ドキュメント

`docs/host-api.md` は、表の `Rubevy.entity` の行（「`Entity::to_bits`」→「`Rubevy::Entity`」）と、
`Answer` の種類、`Request` の読み取り（`entity_arg` を足した）を直し、「エンティティが Ruby からどう見えるか」の
節を新しく足した。`to_i` の値と `inspect` の文字列は実際に動かして確かめた（Bevy 0.19 の `Entity` の `Display` は
`1v0`、その `to_bits` は `4294967294`。索引が反転して入るので、最初のエンティティが `4294967296` だろうという
予想は外れた）。

`docs/rust-bridge.ja.md` は 6 章の 3 項目（関数ポインタと `static`、答えの型、内部フィールド）と、
2 章の表、3.1、3.2 のコード片を直した。6 章の「直した」項目は消さずに取り消し線で残してある
（この文書は「どこが滑らかでないか」を時系列で追う性格なので、何がいつ直ったかが読めるほうがよい）。
`Answer::List` / `Rows` の中のエンティティは今も `f64` 経由であることを明記した。ここが残った制限で、
消すには表の型（`Vec<Vec<f64>>`）を変えることになり、ゲーム側の `radar` の作りに直接響く。

README は host API の行（エンティティがオブジェクトであること、キューが VM の中にあること）に加えて、
「Try it」の VM の入手元を直した（`crates.io` の 0.3 と書いてあったが、`Cargo.toml` は git を指しており、
今回使う `define_closure` などは 0.4.0 にも入っていない。`[patch.crates-io]` の例も git の URL 向けに直した）。
これは指示の範囲外の直しなので、不要ならこのコミットから落とせる形（README だけ）にしてある。

**直していないもの**: `docs/outlook.md:72,142` と `docs/outlook.ja.md:191,289` に「ネイティブは関数ポインタ」
「ホスト状態は `static` 経由」という記述が残っている。`outlook` は本体が保守している計画文書なので触っていない。

## 4. 確認

```
cargo test --workspace   → 6 passed（entity.rs 3、replace.rs 3）、0 failed
cargo build --examples   → headless / sensor とも通る
cargo doc --no-deps      → 警告 0
cargo clippy --all-targets → 2 件。いずれも今回触っていない箇所（drain_commands の
                             collapsible_if と examples/headless.rs の type_complexity）で、
                             作業前と同じ。途中で 1 件増やしたが（`ask` の中の入れ子の if）、
                             `match` に直して戻した。
```

例（`cargo run --example headless` / `sensor`）も最後まで動かした。`sensor` は `Rubevy.ask` の往復と
`Arg` の見え方（`scan asked for [Num(40.0)]`）が変わっていないことの確認になる。`headless` では
`assets/scripts/hello.rb` の `"#{Rubevy.entity}"` の出力が数値から `#<Rubevy::Entity 33v0>` に変わった。
スクリプトから見える**唯一の後方非互換**がこれで、エンティティ番号を文字列に埋めていたスクリプトは
`Rubevy.entity.to_i` と書き直すことになる（リポジトリ内では `hello.rb` の 1 行だけ。
rubevy_games のロボットは `Rubevy.entity` を使っていないことを確かめた）。

ベンチは回していない（別の担当が同じ機械で sabiruby を計測中）。ゲーム（rubevy_games）は触っていない。
API の互換は目視で確かめた: `Answer::{Nil,Bool,Num,Text,List,Rows}`、`Request::{num,text,num_or,entity}`、
`ScriptWorld::{take_requests,answer,stats}`、`Script`、`ScriptTask`、`ScriptDone`、`ScriptEnded` は
いずれも形が変わっていない。変わったのは `pending_command_count()` が `&ScriptWorld` を取るようになったことだけで、
これは `#[doc(hidden)]` でリポジトリ内にも rubevy_games にも呼び出しが無い。
