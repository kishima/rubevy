# ECS の橋（段階 A）: コンポーネントに名前で触る

2026-09-15。指示書は `docs/plans/ecs-bridge-plan.md` の段階 A。ブランチ `ecs-bridge`。

## 読んだところ

最初に読んだのは `src/lib.rs` 全体（914 行）。`install_host_api`（797 行あたり）が `Rubevy` モジュールと
`Rubevy::Entity` を作り、`define_closure` で `log`/`spawn`/`despawn`/`entity`/`move_to`/`ask`/`set_position` を
置いている。ネイティブは `&mut Vm` しか受け取らないので、世界に触る操作は `HostState.commands` に積まれ、
`drain_commands` がフレームの最後に実行する。`Rubevy.ask` だけは別で、`HostCommand::Ask` が
`ScriptWorld::requests` に移り、ゲームの system が `take_requests` / `answer` で答える。答えは `Answer`
という平らな enum（`Nil`/`Bool`/`Num`/`Text`/`List`/`Rows`/`Entity`）。

Bevy 側は実物で確かめた。`bevy_ecs-0.19.1/src/reflect/component.rs:205` の `ReflectComponent::reflect` は
`impl Into<FilteredEntityRef<'w,'s>>` を取り、`EntityRef` からの `From` が
`world/entity_access/entity_ref.rs:312` にある。`reflect_mut` は `FilteredEntityMut` を取り、
`world/entity_access/world_mut.rs:2340` に `From<&mut EntityWorldMut>` がある。
`bevy_reflect-0.19.1/src/type_registry.rs:467` に `get_with_short_type_path`、441 行に `get_with_type_path`。
`ReflectRef` の variant は `bevy_reflect-0.19.1/src/kind.rs:183` に 9 つ（Struct/TupleStruct/Tuple/List/Array/
Map/Set/Enum/Opaque、`functions` 機能があると Function も）。計画書の名前はどれも合っていた。

## 計画書の前提と実物が違ったところ

**1. `Vec3` や `Quat` は `Opaque` ではなく Struct だった。** 計画書は「`Vec3`/`Quat`/`Color` などの `Opaque` は
既知のものだけ配列に」と書いているが、`bevy_reflect-0.19.1/src/impls/glam.rs:250` 以下を見ると、glam の型は
`impl_reflect!(struct Vec3 { x: f32, y: f32, z: f32 })` として **名前つきフィールドの構造体**として登録されている。
つまり放っておくと `tf[:translation]` は `{x: 1.0, y: 2.0, z: 3.0}` になる。計画書の到達点は
`{translation: [x, y, z]}` なので、`glam::Vec2`/`Vec3`/`Vec3A`/`Vec4`/`Quat` の 5 つだけ型パスで見て配列に写す
（`src/reflect.rs` の `AS_ARRAY`）。`Color` は bevy_color では enum（`Srgba(Srgba)` など）なので、この表には
入れず、enum の一般則（1 要素の Hash）で出る。

書き戻す側は表が要らなかった。**Array を構造体に当てるときは位置で当てる**という一般則を入れたので、
`[1.0, 2.0, 3.0]` はそのまま `Vec3` の x/y/z に入る。読み側だけ既知の型を並べればよい、という非対称は
意図したもので、`src/reflect.rs` の `AS_ARRAY` の rustdoc に書いた。

**2. `TransformPlugin` は `Transform` を登録しない。** 計画書は例を「`MinimalPlugins` + `TransformPlugin`」で
作れと言っているが、`bevy_transform-0.19.1/src/plugins.rs:23` の `build` は伝播の system を足すだけで
`register_type` を呼ばない。bevy 0.19 では `bevy_app-0.19.1/src/app.rs:119` の `reflect_auto_register` 機能が
入っているビルドだけが派生型を自動登録する。rubevy は `default-features = false` なので入っていない。
そこで `examples/components.rs` と `tests/components.rs` は `register_type::<Transform>()` を明示して呼ぶ。
これは「登録の無い型は見えない」という段階 A の規則そのものなので、例の中でそう説明した。

**3. 計画書の `Rubevy::Entity#[]` は、この VM では読みに使えない。** ここが一番大きい。詳しくは下の節。

## `[]` で読めない理由と、実際に確かめたこと

計画書は「`Rubevy::Entity#[]` などは `define_fn` で `Rubevy::Entity` に足す。Ruby 側のライブラリではなく
Rust 側に置く」と書いている。しかし読みは「聞いて、答えを待つ」形なので、待つ側が要る。
ネイティブは待てない: `sabiruby/src/builtins/ext_task.rs:888` が
`blocking pop cannot be called from within a C function boundary` を投げる。これは段階 6c の worklog
（`2026-09-15-stage6c-proxy.md`）が書いている通りで、`Rubevy::Proxy` が Ruby で書かれているのも同じ理由。
つまり `define_fn` で置いた `[]` は、キューを返すことしかできない（`e[:Transform].pop` になってしまう）。

そこで Ruby 側に置くことにし、ただし「ゲームが `require` するライブラリ」ではなく
**プラグインが VM 起動時に走らせる prelude**（`src/prelude.rb` → `src/prelude.mrb` を `include_bytes!`）にした。
計画書が Rust 側に置けと言った理由（「`Entity` はプラグインのものなので」）はこれで満たされる:
スクリプトは何も require せずに `Rubevy::Entity#get` を持つ。sabiruby 自身の mrblib と同じ作り方で、
`tools/compile_scripts.sh` が `.rb` から `.mrb` を作り、両方をコミットする。

ここまで書いて最初のテストを走らせたら、6 本中 5 本が落ちた。落ち方が変で、`Rubevy.find(:Npc)`（モジュール
メソッド）だけが通り、`e[:Transform]` を使うものが全部落ちる。スクリプトの終了値を出す使い捨てのテストを
書いて確かめると、例外は
`#<RuntimeError: blocking pop cannot be called from within a C function boundary>`。
prelude は Ruby なのに境界に当たっている。

原因は呼び出し側のバイトコードだった。mrbc は引数 1 個の `[]` を `OP_GETIDX` に畳む（mruby の codegen の
最適化）。sabiruby の `src/vm.rs:3207` は

```rust
Op::Getidx => {
    let recv = reg!(a); let idx = reg!(a + 1);
    let v = self.funcall(recv, self.s.aref, &[idx], Value::Nil)?;
```

と `funcall` で回す。`funcall` は `Cci::Skip` のフレームを積む（`src/vm.rs:1893`）ので、
`fiber_check_native`（2102 行）が真になり、その内側の `pop` は拒まれる。段階 6c で `method_missing` が
呼び出し元のフレームで動くようになったのと同じ問題が、`[]` にはまだ残っている。`Op::Setidx` も同じ
`funcall` を使うが、書き込みは待たないので影響が無い。

**だから読みは `e.get(:Transform)`、書きは `e[:Transform] = tf`（と `e.set`）にした。** これは計画書の到達点
（`tf = e[:Transform]`）と違う。VM を直せば `[]` は使えるようになる（`Op::Getidx` を `op_send_vis` に
変えるだけに見える。レジスタの並びは OP_SEND と同じ）。しかし今回は「sabiruby 側の変更は要らない想定」
「本当に要るなら止まって報告する」という指示なので、VM には触らず、報告で著者に判断してもらう。
`get` を今入れておくのは無駄にならない: VM を直しても `get` はそのまま残せて、`[]` が増えるだけになる。

## 決めた点（計画書が選択肢を挙げていたもの）

* **答えの運び方は `answer_value(request, |vm| Value)`。** `Answer` を再帰的な enum にすると、VM が既に
  持っている Hash/Array の組み立てを二度書くことになる。`ScriptWorld::answer_value` はクロージャに
  `&mut Vm` を渡すだけで、`hash_new`/`hash_set`/`ary_new` や `IntoRuby` がそのまま使える。`Answer` は
  平らなまま（ゲームが普通に答えるときはそれで足りる）。rubevy 自身の `answer_components` も、ゲームが
  入れ子の答えを返したいときも、同じ入り口を使う。
* **書き込みは `HostCommand`。** `Rubevy.set_component(entity, name, hash)` はネイティブで、Hash をその場で
  Rust の `RubyData` に読み出して `HostCommand::SetComponent` に積む。`ask` で運ぶ案（`Arg::Value` として
  `Value` を `gc_register` して運ぶ）は捨てた。理由は 2 つ: `Commands` と同じ「後で反映」の約束に合うこと、
  それと Ruby のオブジェクトをフレームをまたいで生かす必要が無いこと（スクリプトはもう先に進んでいるので、
  その間に Hash を書き換えられると書き込む値が変わってしまう）。
* **`Opaque` の写し方は既知の型だけ。** 数値（f32/f64 と整数一式）、`bool`、`String`、`char`、それに
  `Entity`（`Rubevy::Entity` オブジェクトとして。`bevy_ecs-0.19.1/src/entity/mod.rs:148` で
  `#[reflect(opaque)]`）。それ以外は nil。`Entity` を入れたのは、`ChildOf` のようなコンポーネントを読んだ
  ときに数値ではなくエンティティが出てほしいから。

## 設計で足した規則

* **rubevy 自身が答える `ask` の種類は、ゲームに渡さない。** `RESERVED_KINDS`
  （`component.get`/`component.has`/`components`/`entities.with`）は `drain_commands` の中で
  `ScriptWorld::reflect_requests` に振り分けられ、`take_requests` には出ない。計画書は「`answer_components` を
  ゲームの system より前に置く」と書いていたが、Bevy の Update の中でゲームの system との順序は
  決まっていないので、順序ではなく行き先で分けた。テスト `rubevys_own_questions_never_reach_the_game` が
  これを見ている。
* **`answer_components` はフレームの頭（`deliver_answers` の後、`tick_scripts` の前）。** 前フレームの終わりに
  `drain_commands` が作った質問に、スクリプトが動き出す前に答える。だから 1 往復 = 1 フレーム。
* **書き込みは `apply_component_writes`（`drain_commands` の後）。** どちらも `&mut World` を取る排他 system。
  `drain_commands` を排他にする案もあったが、`Commands` で書かれている今の中身を書き換えることになるので、
  後ろに 1 つ足すだけにした。
* **enum の書き込みは unit variant だけ。** `DynamicEnum` で variant を切り替えると、bevy の derive は
  新しい variant を「渡された値から」組み立てる（`bevy_reflect_derive-0.19.1/src/impls/enums.rs:219` の
  `variant_constructors`）。フィールドのある variant はフィールドの型が要るが、Ruby の Hash は型を言わない。
  unit variant はフィールドが無いので正確に切り替えられる。今の variant と同じ名前なら、フィールドは
  その場で書ける。`tests/components.rs` の `a_symbol_switches_an_enum_component` がこれ。
* **書き込みは部分適用。** Hash が名前を挙げたフィールドだけ書く。`e[:Hp] = { current: 4.0 }` は `max` を
  触らない（テストで見ている）。読んで 1 つ直して書き戻す往復が 1 回で済む、という計画書の要求そのもの。

## sabiruby の公開 API で足りなかったもの

* **Hash を Rust から歩く手段が無い。** `hash_new`/`hash_set`/`hash_get`/`hash_delete` はあるが、キーを列挙する
  ものが無い（`Vm` に `hash_keys` も `hash_each` も無い）。`src/reflect.rs` の `read_at` は Ruby 自身の
  `keys` を `funcall` して `ary_vals` で受けている。ネイティブの中で `funcall` するのは許される（待たないので）。
  `Vm::hash_keys(Value) -> Option<Vec<Value>>` があれば 1 行になる。
* `Vm::str_bytes` が `Option<&[u8]>` なのは借用の都合で少し使いにくいが、`to_vec()` で済む範囲。

## 確かめたこと

`cargo test --workspace`: 6 ファイル + doctest、全部通る（components 6、entity 3、futures 2、proxy 4、
replace 3、doc 2）。`cargo build --examples` 通る。`cargo run --example components` は最後まで動いて
`components: 4 steps` のあと `host: the script left its Transform at Vec3(5.0, 2.0, 3.0)` と
`ended: Finished :components_done` を出す（x が 1.0 → 5.0 に 4 回動いた）。
`cargo doc --no-deps` 警告なし。`cargo clippy --all-targets` の警告は 2 つで、どちらも触っていない場所
（`src/lib.rs` の `SetPosition` の入れ子 if と `examples/headless.rs` の型の複雑さ）。最初に書いた
`src/reflect.rs` は `collapsible_if` と `match_like_matches_macro` を 1 つずつ出したので直した。
