# Rust と Ruby をどうつないでいるか

2026-09-14。rubevy と SabiRuby Battle（`sabiruby/rubevy_games` の `sabibots`）で、Ruby のスクリプトと
Rust のゲームがどう接続されているかを、実際のコードに沿ってまとめたもの。後半で、同じことを
C の mruby を組み込んで作った場合と比べ、VM を Rust で書いたことで何が良くなったのか、何を払っているのかを示す。

関連: 可能性の話は `outlook.ja.md`、ホスト API の一覧は `host-api.md`、ブラウザ版は rubevy_games の `docs/web.md`。

## 1. 要点

- **Ruby はエンティティに直接触らない。** Ruby ができるのは「ゲームに質問する」ことだけで、
  エンティティやコンポーネントを読み書きするのは、いつも普通の Bevy のシステムである。
- **その質問と答えが、Rust の普通の値のまま行き来する。** VM は Rust の crate で、`Vm` は Bevy のリソースに
  そのまま入る。Ruby の値は `enum Value`、失敗は `Result`、スクリプトの実行位置は `Vec<(String, u32)>` として手に入る。
  境界に FFI も `unsafe` もない。
- **C の mruby を組み込むと、この境界のすべてが「C の約束を Rust 側で守る」作業になる。**
  生ポインタ、`longjmp` の例外、GC アリーナ、スレッドをまたげない状態、ビルド設定で変わる値の表現、C と Ruby が要るビルド。
  SabiRuby ではそれがなくなり、代わりに速度と互換性と保守を払っている。

## 2. 全体像

SabiRuby Battle で、ロボットが「アクセルを踏んで砲塔を回し、撃つ」とき:

```
Ruby タスク（ロボットの頭脳）            rubevy（プラグイン）                 ゲームのシステム（Rust）
─────────────────────────────           ─────────────────────               ─────────────────────────
act throttle: 1.0, aim: a, fire: 0.3
  └ Rubevy.ask("act", 1.0, -999, a, 0.3)
      ネイティブ: キューを作り、
      HostCommand::Ask を積む ─────────▶ drain_commands
    .pop で停止（CPU を使わない）          └ ScriptWorld.requests へ ──────▶ answer_requests
                                                                             robots.get_mut(entity)
                                                                             robot.throttle = 1.0 …
                                                                             弾を spawn、エネルギーを減らす
                                         task_queue_push ◀─────────────── world.answer(Answer::List([1, 42, 0.4]))
    ◀─ 次のスケジューラの番で再開、
       pop が [1.0, 42.0, 0.4] を返す
```

接点は 6 つある。

| 接点 | Rust 側 | Ruby 側 |
|---|---|---|
| 起動 | `Script` コンポーネント → `start_scripts` が `task_spawn`、タスクにエンティティ番号を持たせる | ファイルのトップレベルがタスクとして動く |
| 質問 | `define_closure` で登録したネイティブ `Rubevy.ask` | `Rubevy.ask("radar", 60.0).pop` |
| 回答 | `ScriptWorld::take_requests` / `answer`（`Answer` enum） | `pop` の戻り値（Float、配列、配列の配列…） |
| 時間 | `tick_scripts` が Bevy の `Time` でスケジューラの時計を進め、命令数の予算で回す | `sleep 0.05` が実時間で起きる |
| 観察 | `ScriptWorld::stats` → `task_instructions` / `task_frames` | （何もしなくてよい） |
| 停止 | `ScriptTask` の `on_remove` フックが `Task#terminate` | タスクが止まる |

## 3. 各段の詳細

### 3.1 起動: エンティティとタスクを結ぶ

`Script` を付けたエンティティは、`.mrb` のアセットが届いた時点でタスクになる（`rubevy/src/lib.rs` `start_scripts`）。

```rust
let irep = vm.load(&asset.bytes)?;
let task = vm.task_spawn(irep, script.priority, Some(&name))?;
vm.gc_register(task);                                  // Rust 側が持つので GC に回収させない
let k = vm.intern("@rubevy_entity");
vm.heap.ivar_set(task, k, Value::Int(entity.to_bits() as i64));
commands.entity(entity).insert(ScriptTask { task });   // ObjId（u32）を普通のコンポーネントに
```

- タスク（Ruby の `Task` オブジェクト）の ID は `ObjId(u32)` という Copy な値で、コンポーネントにそのまま入る。
- エンティティ番号はタスクのインスタンス変数に置く。ネイティブは「いま走っているタスク」からそれを読むので、
  Ruby のコードは自分がどのエンティティかを意識しない。Ruby に渡すときは `Rubevy::Entity` という
  Data オブジェクト（`Vm::data_new`、ハンドルが `to_bits`）に包む。`Rubevy.entity` が返すのがそれで、
  `to_i` で番号、`==` はハンドル比較、`dup` は `TypeError`。
- SabiRuby Battle は `.rb` をゲームの中でコンパイルする（PC 版は同じプロセスの `sabiruby-compiler`、ブラウザ版は JS 経由）。
  ロボットのファイルの前に DSL（`prelude.rb`）をつなげて 1 本のプログラムにしている。

### 3.2 質問: ネイティブメソッドは Rust の関数

`Rubevy.ask` の実体（`install_host_api`）:

```rust
vm.define_closure(sc, "ask", move |vm, _self, a, _blk| {
    let kind = String::from_utf8_lossy(&vm.as_string(a[0])?).into_owned();  // 型が違えば TypeError が `?` で Ruby に戻る
    let args: Vec<Arg> = a[1..].iter().map(|v| match v {
        _ if is_entity(vm, *v) => Arg::Entity(/* Data のハンドル = to_bits */ …),
        Value::Int(i)   => Arg::Num(*i as f64),
        Value::Float(f) => Arg::Num(*f),
        Value::Sym(s)   => Arg::Text(vm.sym_name(*s).to_string()),
        other           => /* 文字列なら Text */ …,
    }).collect();
    let queue = vm.task_queue_new()?;          // mruby-task の Task::Queue
    vm.gc_register(queue);
    let entity = current_entity(vm);           // vm.task.running の @rubevy_entity
    push_command(vm, HostCommand::Ask { entity, kind, args, queue });  // キューは VM の host_state の中
    Ok(Value::Obj(queue))
});
```

- ネイティブの型は `fn(&mut Vm, Value, &[Value], Value) -> VmResult<Value>`。引数は `&[Value]`、戻り値は `Result`。
  `define_closure` はこれをクロージャとして取る（`Arc<dyn Fn …>`）ので、ホストの値を捕まえられる。
  ここでは `Rubevy::Entity` クラスの `ObjId` を捕まえている。
- `Value` は `enum { Nil, False, True, Int(i64), Float(f64), Sym(Sym), Obj(ObjId) }` なので、引数の解釈は `match` で書ける。
  取りこぼしはコンパイラが指摘する。
- 例外は `Err(VmError::Raise(..))` という値で、`?` で Ruby に戻る。Rust の関数の途中を飛び越えないので、
  途中で作った `String` や `Vec` の後始末（`Drop`）は必ず走る。
- 質問はキューに積むだけで、ここではゲームの世界に触らない。触れない理由は 6 章に書く。

Ruby 側は `pop` で待つ。`Task::Queue#pop` は空ならタスクを止め、スケジューラは他のタスクに移る。
待っている間、このロボットは CPU を使わない。

```ruby
def radar(range = 60.0)
  rows = Rubevy.ask("radar", range).pop
  @me = Status.new(rows.shift)
  rows.map { |row| Contact.new(row, @me.team) }
end
```

### 3.3 回答: ゲームのシステムが普通のクエリで答える

`drain_commands` が質問を `ScriptWorld` に移し、ゲームのシステム（`sabibots/src/main.rs` `answer_requests`）が取り出す。

```rust
fn answer_requests(mut world: ResMut<ScriptWorld>, mut robots: Query<(Entity, &mut Robot, &Transform)>, …) {
    for request in world.take_requests() {
        let Some(me) = request.entity else { … };
        let Ok((_, mut robot, transform)) = robots.get_mut(me) else { … };
        let answer = match request.kind.as_str() {
            "radar" => Answer::Rows(/* 周りのロボット 1 台 1 行、ノイズを足す */),
            "act" => {
                if let Some(t) = request.num(0).filter(|v| *v > UNSET) { robot.throttle = (t as f32).clamp(-1.0, 1.0); }
                …
                Answer::List(vec![fired, robot.energy as f64, robot.cooldown as f64])
            }
            …
        };
        world.answer(&request, answer);
    }
}
```

`ScriptWorld::answer` が `Answer` を Ruby の値にしてキューに入れる。

```rust
Answer::Rows(rows) => {
    let items = rows.into_iter()
        .map(|row| self.vm.ary_new(row.into_iter().map(Value::Float).collect()))
        .collect();
    self.vm.ary_new(items)
}
…
self.vm.task_queue_push(request.queue, value)?;   // 待っているタスクが実行可能になる
self.vm.gc_unregister(request.queue);
```

- ゲームの規則は全部 Rust 側にある。Ruby が頼めるのは「スロットルを 1.0 に」まで。
  範囲の制限、エネルギーの消費、撃てるかどうかはシステムが決める。
- VM を動かしている最中に Bevy の `World` を借りる必要がない。借用規則とぶつからず、並列実行の妨げにもならない。
- 答えを後回しにしてもよい。`Request` を持っておき、数フレーム後に `answer` すれば、その間 Ruby は待つだけ
  （経路探索やアセット読み込みに向く。いまのゲームはすべて同じフレームで答えている）。

### 3.4 時間: Bevy の時計でスケジューラを回す

`tick_scripts`（毎フレーム）:

```rust
world.vm.task_advance_ticks(whole_ticks);   // Bevy の Time で mruby-task の時計を進める（task_external_clock(true)）
set_frame_state(&mut world.vm, …);          // $rubevy = { frame:, delta:, time: }
world.vm.task_run_budget(budget)?;          // 走れるタスクを、命令数の予算の範囲で回す
```

- `sleep 0.05` は実時間で起きる。時計はホストが渡すので、テストでは時間を好きに進められる。
- 時計とは別に、命令数でタイムスライスが切れる。`loop {}` と書いたロボットは自分の番を失うだけで、フレームは止まらない。
- 命令数では止められないもの（ネイティブの中で待たれているブロック、`sort { }` や `Array.new(1) { loop { } }`）に備えて、
  フレームには時間の上限もかかる（`task_run_limits`、時計は Bevy の `Instant`）。8ms を超えると走っているスライスを打ち切り、
  50ms を超えても切り替えられないタスクには `Task::Overrun` を投げる。スライス自体は命令数のままなので、動きはマシンによらない。
- GC はスケジューラの暇な時点で回す（`GC.scheduler_driven = true`）。スクリプトが割り当てた瞬間にフレームが止まることがない。
- 捕まえていない例外はタスクの結果になり、`ScriptEnded { status: Failed }` として通知される。他のロボットは動き続ける。

### 3.5 観察: VM の中身を Rust の値として読む

エディタの行の色付けと、スコアボードの「thinking」は、VM の状態を毎フレーム読んで作っている。

```rust
let stats = world.stats(script);   // instructions: u64, location: Option<(String, u32)>, frames: Vec<(String, u32)>
panel.spent = stats.instructions - robot.last_instructions;
robot.own_line = stats.frames.iter()
    .find(|(_, line)| *line > robot.prelude_lines)      // DSL の中で待っていても、ロボット自身のファイルの行を探す
    .map(|(_, line)| line - robot.prelude_lines);
```

`task_frames` は、止まっているタスクのフレームを内側から順に、ファイル名と行の組で返す。
中身は VM の構造体（コンテキスト、コールインフォ、デバッグ情報）を読んでいるだけで、
この API は必要になった日に VM へ小さな関数として足した。

### 3.6 停止: コンポーネントを外せばタスクが止まる

```rust
#[derive(Component)]
#[component(on_remove = stop_removed_task)]
pub struct ScriptTask { task: ObjId }

fn stop_removed_task(mut world: DeferredWorld, ctx: HookContext) {
    …
    scripts.vm.funcall(Value::Obj(task), terminate, &[], Value::Nil)?;   // Task#terminate
    scripts.vm.gc_unregister(task);
}
```

エンティティを消す、`ScriptTask` を外す（ホットリロード、Apply、リスタート）、ロボットが倒れる。
どの場合もタスクが止まる。Bevy のフックから、Ruby のメソッドを普通の関数として呼んでいる。
これが入る前は、差し替えた古い頭脳が VM の中で動き続けていた（`tests/replace.rs` が回帰テスト）。

## 4. C の mruby をつないだ場合との比較

同じ rubevy を、C の mruby と Rust のバインディング（`bindgen` などで生成）で作ったとする。
「できない」ことは少ない。mruby は組み込み用に作られた VM で、C の API はよく整っている。
違いは、**境界のひとつひとつが、Rust のコンパイラが確かめてくれない約束になる**ことである。

### 4.1 一覧

| 観点 | C の mruby を組み込む | SabiRuby |
|---|---|---|
| VM を持つ | `*mut mrb_state`。生ポインタなので `Send` でも `Sync` でもない | `Vm` は `Send + Sync` の普通の値 |
| Bevy に置く | `NonSend` リソース（使うシステムはメインスレッドに固定）か、`unsafe impl Send` で約束する | 普通の `Resource`。`ResMut<ScriptWorld>` とクエリを同じシステムで使える |
| 値 | `mrb_value`。中身はビルド設定（ワード／NaN／ボックス化なし）で変わり、`mrb_fixnum_p` などのマクロで調べる。バインディングは同じ設定で生成しないと壊れる | `enum Value` を `match` |
| ネイティブの引数 | `mrb_get_args(mrb, "z*", …)` の書式文字列。書式と変数の型の食い違いはコンパイル時に分からない | `&[Value]` と `vm.expect_int` など。型は Rust が確かめる |
| 例外 | `mrb_raise` は `longjmp`（C++ ビルドなら C++ 例外）。Rust のフレームを `longjmp` で飛び越えると、`Drop` を持つ値がある限り未定義動作 | `Err(VmError)` を `?` で返す。`Drop` は必ず走る |
| Rust の panic | `extern "C"` の関数から C のフレームを越えて巻き戻せない（現行の Rust では abort）。ネイティブごとに `catch_unwind` が要る | 普通の Rust の panic。ホストで `catch_unwind` もできる |
| GC とネイティブ | ネイティブで作ったオブジェクトは GC アリーナで守られ、ループで大量に作ると溢れる（`mrb_gc_arena_save`/`restore` が要る） | ネイティブ実行中は回収しない（`native_active`）。アリーナの管理はない |
| ホストが持つ Ruby の値 | `mrb_gc_register` | `vm.gc_register`（同じ考え方） |
| Rust の値を Ruby に持たせる | `RData` と `mrb_data_type` の `dfree` に、`Box::into_raw`／`from_raw` を手で対応させる | 同じ仕組み（`ObjKind::Data`）を Rust の型で持てる（ECS の橋で使う予定。`outlook.ja.md`） |
| 実行位置、命令数、フレーム | 内部構造体（`mrb_context`、`mrb_callinfo`）を生ポインタでたどる。mruby-task のタスク構造は gem の内部 | `task_frames` などが所有権のある値を返す。足りなければ同じリポジトリに足す |
| VM を直す | C の gem を直し、バインディングを作り直し、ビルドし直す | Rust の関数を 1 つ足す（このプロジェクトで何度もやった） |
| ビルド | mruby のビルドには CRuby と rake と C コンパイラが要る。クロスコンパイルは mruby の `build_config` で行う | `cargo build`。VM に C は要らない |
| ブラウザ | C の VM を wasm にするには、emscripten か wasi-sdk と、`setjmp` のための例外処理対応が要る。Bevy の wasm-bindgen 出力と 1 つのモジュールにまとめるのは難しい | VM は `wasm32-unknown-unknown` にそのままビルドされ、ゲームと同じモジュールに入る |
| メモリ安全 | VM 全体が C。GC や境界の誤りは黙ってメモリを壊しうる（mruby はファジングで多数の報告を受け、直してきた） | VM の crate の `unsafe` は 1 か所（命令のバイト値を enum にする箇所。末尾の補足）。ほかの不具合は「間違った値」「例外」「panic」 |

### 4.2 同じ `Rubevy.ask` を C の mruby で書くと

ネイティブ本体（C で書くか、Rust の `extern "C"` で書く）:

```rust
// Rust で書く場合。すべて unsafe。
extern "C" fn rubevy_ask(mrb: *mut mrb_state, _self: mrb_value) -> mrb_value {
    // 1. 引数は書式文字列で取る。"z*" と変数の型が合っているかはコンパイラが見ない
    let mut kind: *const c_char = ptr::null();
    let mut rest: *const mrb_value = ptr::null();
    let mut n: mrb_int = 0;
    unsafe { mrb_get_args(mrb, c"z*".as_ptr(), &mut kind, &mut rest, &mut n) };
    // 2. mrb_get_args は型が違えば longjmp で戻る。ここより上に Drop を持つ値を置いてはいけない
    // 3. Rust の panic をここで止めないと abort する
    let result = std::panic::catch_unwind(|| {
        let args = unsafe { std::slice::from_raw_parts(rest, n as usize) };
        let args: Vec<Arg> = args.iter().map(|v| unsafe {
            // mrb_value の中身の判定は、ビルド設定ごとのマクロを移植した関数で行う
            if mrb_fixnum_p(*v) { Arg::Num(mrb_fixnum(*v) as f64) } else { … }
        }).collect();
        // 4. ホストの状態は mrb->ud（void*）から取る。型は自分で覚えておく
        let host = unsafe { &mut *((*mrb).ud as *mut HostState) };
        …
    });
    // 5. Ruby の例外を投げるなら、Rust の値をすべて片付けてから mrb_raise（longjmp）
    …
}
```

回答で、表（配列の配列）を作る側:

```c
mrb_value rows = mrb_ary_new_capa(mrb, n);
int ai = mrb_gc_arena_save(mrb);             // これを忘れると、行が多いとアリーナが溢れる
for (int i = 0; i < n; i++) {
  mrb_value row = mrb_ary_new_capa(mrb, 11);
  for (int j = 0; j < 11; j++) mrb_ary_push(mrb, row, mrb_float_value(mrb, cells[i][j]));
  mrb_ary_push(mrb, rows, row);
  mrb_gc_arena_restore(mrb, ai);             // row は rows から辿れるので戻してよい
}
```

SabiRuby の同じ処理は、3.2 と 3.3 に載せたコードで全部である。
書き方の問題に見えるが、実際には「間違えたときに何が起きるか」が違う。
C 側の約束を破ると、クラッシュ、メモリ破壊、たまにしか起きない不具合になる。
SabiRuby で同じ種類の間違いをすると、多くはコンパイルが通らず、通っても `Err` か panic になる。

### 4.3 このプロジェクトで実際に効いたところ

比較表の中から、SabiRuby Battle を作る途中で実際に役立ったものを挙げる。

1. **VM を普通のリソースに置けた。** `answer_requests` は `ResMut<ScriptWorld>` とロボットのクエリを 1 つのシステムで使っている。
   `NonSend` だと、VM に触るシステムがすべてメインスレッドに固定され、描画や入力と同じ列に並ぶ。
2. **足りない API をその日のうちに足せた。** 実時間の `sleep`（`task_external_clock`、`task_advance_ticks`）、
   1 フレーム分だけ回す `task_run_budget`、エディタの行表示のための `task_location` と `task_frames`、
   ネイティブが値を返しながらタスクを止める `park()` の修正。どれも VM の Rust の関数を足すか直す作業で、
   ホストからは所有権のある戻り値として使えた。C の mruby なら、C の gem の内部を公開し、バインディングを作り直すことになる。
3. **止める処理が Bevy のフックに収まった。** `on_remove` から `funcall` で `Task#terminate` を呼ぶ。
   フックの中は `DeferredWorld` で、VM は `get_resource_mut` で取れる普通のリソースである。
4. **テストが Rust の中で完結した。** `tests/replace.rs` は Bevy の `App` と VM を同じテストで動かし、
   差し替え前後の質問の数を数えている。C の VM を持ち込むと、テストのビルドにも C のツールチェーンが要る。
5. **ブラウザ版が作れた。** ゲームの wasm は VM ごと `wasm32-unknown-unknown` でビルドされ、C の標準ライブラリを必要としない。
   C で書かれているのは、実行中に `.rb` をコンパイルするコンパイラだけで、それは Playground のモジュールとして別に読み込んだ。
   （あらかじめコンパイルした `.mrb` だけで遊ぶゲームなら、C は 1 行も要らない。）

## 5. SabiRuby 側の代償

良い点だけを書くと比較にならないので、払っているものも書く。

- **速度。** 本家 mruby 4.1.0-rc との比較（`sabiruby/docs/verification/bench.md`）で、2026-09-15 の高速化の後で全体 3.0 倍、fib 3.1 倍、
  `so_lists` 4.4 倍、Hash 7.7 倍遅い（経緯は `sabiruby/docs/design/optimizations.md`）。
  rubevy は Ruby を「毎フレーム大量に計算する言語」ではなく「判断を書く DSL」として使う方針なので、効きにくい弱点だが、弱点ではある。
  SabiRuby Battle のロボットは 1 フレームに数百命令しか使っていない。
- **互換性。** 本家のテストスイートは 2507 件中 2344 件が通る（残りは理由付き）。本家の C で書かれた gem（mruby-io、mruby-socket など）は使えない。
  gem は 1 つずつ Rust に移植している。
- **コンパイラは C のまま。** Prism とコード生成は本家の C（`sabiruby-compiler`）。ソースを読む段階は Rust の保証の外で、
  実行中にコンパイルするゲームは C のツールチェーンを必要とする（ブラウザ版では wasm モジュールを 2 つにして避けた）。
- **追従の手間。** 本家の変更は自動では入らない。mruby-task はほぼフォークとして扱うと決めた（`sabiruby/docs/gems.md`）。

## 6. まだ滑らかでないところ

- ~~**ネイティブはクロージャではなく関数ポインタ。**~~ **直した**（2026-09-15）。VM 側に `define_closure`
  （`Arc<dyn Fn … + Send + Sync>` を取るネイティブ）と `set_host_state` / `host_state_mut::<T>()`
  （`Box<dyn Any + Send + Sync>` を型付きで出し入れする）が入ったので、`static` の `Mutex<Vec<HostCommand>>` は
  `Vm` の中の `HostState` になった。`Rubevy` のメソッドは `define_closure` で登録し、
  `Rubevy::Entity` クラスの `ObjId` をクロージャに捕まえている。
  同じプロセスで `App` を 2 つ動かしても取り合わないので、`tests/replace.rs` は 3 本の `#[test]` に戻した。
  古い実装に新しいテストを当てると「質問の数が合わない」どころか
  `access to freed object ObjId(667)` で落ちる: `Request` が持つキューの `ObjId` は VM ごとのヒープの添字で、
  VM をまたぐと別のものを指す。static を使うということは、その約束を型で守れないということでもあった。
- ~~**VM の内部に直接触っている。**~~ **直した**（2026-09-15）。`vm.heap.ivar_set` / `ivar_get`、
  `vm.task.running`、`vm.globals.insert`、`vm.heap.get(o).kind` の 4 か所（タスクにエンティティ番号を
  持たせる、走っているタスクを知る、`$rubevy` を毎フレーム置く、終わったタスクの結果が例外か）は、
  VM 側に入った `Vm::ivar_set` / `ivar_get`、`task_running`、`global_set` / `global_get`、
  `is_exception` に置き換わった。`grep -rn "vm\.heap\|vm\.task\|vm\.globals" src/` は 0 件で、
  `sabiruby::value::Slot` と `sabiruby::object::ObjKind` の import も消えた。
  `is_exception` は Ruby の `is_a?(Exception)` を呼ぶのではなくオブジェクトの表現を見るので、
  判定のために VM に再入しない（再定義された `is_a?` にも影響されない）。
- **答えの型が狭い。** `Answer` は数値・文字列・数値の配列・その表、そしてエンティティ。
  ~~エンティティ番号も `f64` で渡している~~ **エンティティは直した**（2026-09-15）。`Rubevy::Entity` という
  Data オブジェクト（`Vm::data_new`。ハンドルが `Entity::to_bits`、VM はその中身を読まない）にし、
  `Answer::Entity` と `Arg::Entity` を足した。`to_i` で番号、`==` はハンドル比較、`dup`/`clone` は `TypeError`。
  `Answer::List` / `Rows` は数値の表のままにしてある（SabiRuby Battle の `radar` が 1 台 1 行の数値の表を返しており、
  表の 1 セルだけオブジェクトにする形は型に入らない）ので、**表に載るエンティティは今も `f64` 経由**で、
  そこは 2^53 の制限が残っている。`Rubevy.despawn` / `set_position` はどちらの形も受ける。
  数値・文字列・エンティティ以外（構造体、コンポーネント）を渡す形は、`Data` の仕組みが使えるようになったので
  次はホスト側が種類を足すだけになった。
- **1 回の質問に 1 フレーム前後かかる。** 質問はシステムの順で処理されるため。`radar` が自分の状態も返し、
  `act` が操作をまとめて送るのはこの遅れを減らすため。
- **ECS の橋はまだ。** エンティティを Ruby のオブジェクトとして包むところまでは済んだ（上の `Rubevy::Entity`）。
  残りの「Bevy のリフレクションでコンポーネントに名前で触る」形は `outlook.ja.md` の計画のまま。今の「質問して答える」形は、その橋ができても、ゲームの規則を Rust に閉じ込めたい場面では残る。

## 補足: `unsafe` の数について

VM の crate の `unsafe` は 1 か所で、命令のバイト値を範囲確認したうえで enum に変換する箇所（`src/opcode.rs`）。

一時期（2026-09-13〜14）は 15 か所あった。正規表現の移植が、本家の C の形（`DATA_GET_PTR` でパターンの
ポインタを取り、検索に使い続ける）をそのまま `*const Pattern` にし、14 か所で `unsafe { &*pat }` と読み戻していたため。
検索の途中で VM を `&mut` で使う（MatchData を作る、`$~` を置く、`gsub` ではブロックで任意の Ruby を動かす）ので、
ヒープから借りた `&Pattern` を持ったままにできない、というのが理由だった。

そのときも壊れてはいなかったが、安全は「Regexp は 2 回初期化できない」「ネイティブの実行中は GC が回収しない」という、
コンパイラが確かめない 2 つの約束に頼っていた。2026-09-14 に、Regexp がパターンを `Arc<Pattern>` で持つように変え、
各メソッドの冒頭で `Arc` を複製するだけにした（呼び出し 1 回につき参照カウント 1 回）。
検索が持っている間はオブジェクトに何が起きてもパターンは生きているので、約束に頼る必要がなくなった。
本家のテスト（文字列ビルド・バイト列ビルドの両方）は、正規表現のファイルを含めて基準から減っていない。

C の VM を Rust から使う場合は、この「ポインタを持ったまま VM を呼び戻す」形が境界のあちこちに現れ、
Rust 側では同じ置き換えができない（C のオブジェクトの寿命は C の GC が決める）。4 章の比較の一例でもある。
