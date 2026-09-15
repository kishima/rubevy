# ECS の橋とイベント（実装指示書）

作成 2026-09-15。sabiruby の `docs/plans/host-bridge-plan.md`（段階 0〜6、済み）の続きで、rubevy 側の仕事。
著者の判断: **A（リフレクションの橋）→ B（イベント）の順**。読み取りはまず「質問して答えを待つ」形（1 往復 ≈ 1 フレーム）で作り、
排他システムの中で同期に読む形は、遅さが問題になったときに足す。

土台（済み）: `Rubevy::Entity`（`Data` オブジェクト、`Entity::to_bits` を持つ）、`Rubevy.ask` と `ScriptWorld::take_requests`/`answer`/`answer_with`、
`Answer::{Nil,Bool,Num,Text,List,Rows,Entity}`、`Rubevy::Proxy`（`method_missing` → `ask`。VM 側で同じフレームに入るので中で待てる）、
型付きホスト状態、`define_fn`。`outlook.ja.md` の「ECS のエンティティは Ruby のオブジェクトになるか」の 2 段目がこの計画。

## 状況

| 段階 | 内容 | 状態 |
|---|---|---|
| A | コンポーネントに名前で触る（`Reflect` 経由、構造ごと 1 往復） | **済み**（2026-09-15、`0cf047c`）。読みは `e[:Transform]`（VM の `OP_GETIDX` を本家と同じ送信にした、sabiruby `ad54ed4`） |
| B | イベントの受け口（observer → `Task::Queue`） | **済み**（2026-09-15、`601b889`）。上限 64、購読解除は `ScriptTask` の除去とタスク終了の 2 か所 |

## 段階 A: コンポーネントに名前で触る

**到達点**（Ruby から見た形）:

```ruby
e = Rubevy.entity
tf = e[:Transform]                 # Hash: {translation: [x, y, z], rotation: [x, y, z, w], scale: [...]}
tf[:translation][0] += 1.0
e[:Transform] = tf                 # 書き込みはコマンドとして後で反映（Commands と同じ約束）
e.has?(:Velocity)                  # true / false
e.components                       # ["Transform", "Sprite", ...] （型の短い名前）
other = Rubevy.find(:Npc)          # マーカーコンポーネントを持つエンティティの配列（Rubevy::Entity）
```

**設計**（型ごとの接着コードを書かない。Bevy 0.19 のリフレクションで名前から辿る）:

* **答え方**: `ask("component.get", entity, "Transform")` を rubevy 自身のシステム（`answer_components`、ゲームのシステムより前）が答える。
  `AppTypeRegistry` を `get_with_short_type_path`（無ければ `get_with_type_path`）で引き、`ReflectComponent::reflect(entity_ref)` で `&dyn PartialReflect` を取り、
  **`PartialReflect` を Ruby の値に写す**変換（`reflect_ref()`: Struct → Hash（キーはシンボル）、TupleStruct/Tuple/List/Array → Array、
  Enum → シンボルか `{variant: …}`、Value → Float/Int/Bool/String、`Vec3`/`Quat`/`Color` などの `Opaque` は既知のものだけ配列に）を書く。
  変換は `src/reflect.rs` に置き、`Answer` に新しい variant `Answer::Value(RubyValue)` 相当を足すか、`Answer::Hash`/入れ子の `List` を足す
  （今の `Answer` は数値の配列と表だけ。**入れ子の値**が要るので、`Answer` を再帰的な enum にするか、`ScriptWorld::answer_value(request, |vm| Value)` の
  形でホストに `Vm` を渡して直接組ませるか。後者のほうが単純で、`FromRuby`/`IntoRuby` がそのまま使える。理由を worklog に書いて決める）。
* **書き込み**: `e[:Transform] = hash` は `ask("component.set", entity, "Transform", hash)` … だが `Arg` は数値と文字列しか運べない。
  `Rubevy.ask` の引数に Ruby の Hash/Array を許す（`Arg::Value` として VM の `Value` を `gc_register` して運び、答えた後に解除）か、
  書き込み専用の `Rubevy.set_component(entity, name, hash)` を `HostCommand` にして `drain_commands` が `ReflectComponent::apply` で反映するか。
  後者が `Commands` の約束（後で反映）に合う。読んだ値を直して戻す往復が 1 回で済むように、`apply` は `PartialReflect` の部分適用（Hash に無いフィールドは触らない）。
* **登録**: リフレクションで触れるのは `Reflect` + `#[reflect(Component)]` で登録された型だけ。Bevy 標準の `Transform`/`Sprite`/`Visibility` は登録済み。
  ゲームの型はゲームが `app.register_type::<T>()` する。登録の無い型は `nil` と、`e.components` に出ない理由を rustdoc に。
* **`Rubevy.find(:Npc)`**: `ask("entities.with", "Npc")`。`ReflectComponent` の `contains` で全エンティティを走査する（毎フレーム呼ぶ用途ではないと rustdoc に）。
* **`Rubevy::Entity#[]`** などは `define_fn` で `Rubevy::Entity` に足す（`src/lib.rs` の `install_host_api` の隣）。Ruby 側のライブラリではなく Rust 側に置く
  （`Entity` はプラグインのものなので）。
* **1 往復 ≈ 1 フレーム**の遅れはこの段階では受け入れる。rustdoc に「毎フレーム大量に読む形ではなく、宣言時とイベント時に触る」と書く。

**確認**: `tests/components.rs`（`Transform` の読み書き、`has?`、`components`、登録の無い型は nil、`find`）。`examples/components.rs`（`MinimalPlugins` + `TransformPlugin` で、
スクリプトが自分の `Transform` を読んで動かす）。rubevy_games には触らない。

## 段階 B: イベントの受け口

**到達点**:

```ruby
hits = Rubevy.subscribe(:hit)          # Task::Queue
Task.new(name: "reflex") do            # 別タスクで待つ（SabiRuby Battle の reflex がこれ）
  loop { by, damage = hits.pop; log "hit by #{by} for #{damage}" }
end
```

**設計**（新しい VM の機構は要らない。`Task::Queue` に流すだけ）:

* Rust 側: `ScriptWorld::publish(entity: Option<Entity>, name: &str, payload: Answer)`。`entity` が `Some` ならそのエンティティのスクリプトが購読しているキューへ、
  `None` なら購読している全部へ。ゲームのシステムや observer（`app.add_observer(|on: On<Hit>, mut world: ResMut<ScriptWorld>| …)`）がこれを呼ぶ。
  Bevy のイベント型を自動で結ぶことはしない（型ごとにゲームが 1 行書く。リフレクションで自動化するのは後）。
* Ruby 側: `Rubevy.subscribe(name)` はネイティブで、`task_queue_new` してホスト状態の購読表 `(entity, name) -> Vec<queue>` に登録し、キューを返す。
  購読はスクリプトのタスクが終わったとき（`ScriptTask` の除去）に外す。キューが溢れないよう、購読者が読まない間に溜まる上限（例 64）を決めて古いものから捨てる
  （理由を rustdoc に）。
* `examples/events.rs` と `tests/events.rs`（購読したものだけ届く、エンティティ宛と全体宛、別タスクで待てる、スクリプトが終わると購読が消える）。
* SabiRuby Battle の `reflex` はこの上に載るが、この計画の範囲外（rubevy_games 側で後日）。

**確認**: `cargo test --workspace`、examples、docs（`host-api.md` に 2 節、`README.md`、`docs/README.md`、`rust-bridge.ja.md` の該当箇所、`outlook.md`/`outlook.ja.md` の
「ECS の橋」と「イベント」の状態）。ベンチ不要。

## 記録

* 過程は `docs/worklog/2026-09-15-ecs-bridge.md`（A）と `2026-09-15-events.md`（B）。
* 終わったらこの文書の「状況」を本体が更新する。
