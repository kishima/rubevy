# 2026-09-16 橋の続き（VM の新しい入り口、子タスクの継承、購読の終わり方）

`docs/plans/ecs-bridge-plan.md` が終わったあとに、2 人の担当が残していった 3 つを片づけた記録。
ブランチ `bridge-followups`（rubevy main `6fe63e0` から）。出所は
sabiruby `docs/worklog/2026-09-16-leftovers.md` の項目 8（rubevy 側）と、
rubevy_games `docs/worklog/2026-09-16-reflex.md` の「壊れたところ 2」「`ensure` は外から終わらされると走らない」。

最初に `cargo update -p sabiruby -p sabiruby-compiler` で VM を main（`1ae258f`）に上げた（`11bcaa0`）。
`hash_keys` / `task_queue_len` / `task_queue_try_pop` はそこで入ったもので、1 番はそれを使うだけの話。

## 1. `funcall` で代用していた 3 か所

sabiruby の worklog が数えたとおり、実際の呼び出し元は 3 か所だった。

`src/reflect.rs` の Hash を歩くところは、コメント自体が
「The VM has `hash_new`, `hash_set` and `hash_get` but no way to walk a Hash from Rust」と
無いことを書いていた。`vm.hash_keys(other)` は `Option<Vec<Value>>` を返すので、
その前にあった `vm.obj_is_kind_of(other, vm.core.hash)` の判定ごと `if let Some(keys) = …` に畳める。
判定の意味は変わらない（Hash の部分クラスのインスタンスもヒープ上の kind は `Hash` なので、
`hash_keys` は同じように答える）。消えたのは送信 1 本と、そのために Ruby のヒープに作られて
1 回読まれて捨てられる Array 1 本。

`ScriptWorld::make_room`（`src/lib.rs`）の方が効く場所で、ここは **publish のたびに**——
つまり `Rubevy.publish` を毎フレーム呼ぶゲームでは毎フレーム——回る。
前は `size` を送って Array の長さを読み、あふれていたら `__pop_try(true)` を送っていた。
`task_queue_len` と `task_queue_try_pop` はどちらもキューの `@items` を直に読むので、フレームを積まない。

`__pop_try(true)` は空のキューで `Task::Error` を上げる（ホストから見ると「今は無い」は例外ではない）ので
前のコードは `.is_err()` で「取れなかった」を見ていた。`task_queue_try_pop` は `Ok(None)` なので
そのまま `match` で書ける。`n >= QUEUE_LIMIT` の間は必ず 1 つ以上あるから `None` は起きないが、
そこで `return` するのがこのループが必ず終わる理由なので、握りつぶさずに書いた。

確認は `cargo test --workspace`（30 + doctest 6、全部通る）。この 3 か所は
`tests/components.rs`（Hash を書き戻す道）と `tests/events.rs`（64 件の上限）が通っている。

## 2. `Task.new` で作ったタスクが、作った側のエンティティを引き継ぐ

rubevy_games の reflex の worklog が「壊れたところ 2」に書いた穴。
`Rubevy.ask` も `Rubevy.subscribe` も `Rubevy.entity` も `current_entity(vm)`（`src/lib.rs`）を読み、
それは `vm.task_running()` が答えるタスクの `@rubevy_entity` を見るだけ。
プラグインがその ivar を付けるのは `start_scripts` が `task_spawn` したタスクだけなので、
スクリプトが `Task.new` で作ったタスクには何も無い。結果、子タスクの `ask` はエンティティ無しの
リクエストになってゲーム側で握りつぶされ、`subscribe` は
「subscribe from the script's own task (a Task.new task has no entity)」で断られていた。
ゲームの側は手で `t.instance_variable_set(:@rubevy_entity, …)` を書いて逃げていた（`prelude.rb:248`）。

**VM に親を持たせるか、rubevy で写すか。** SabiRuby の `TaskData`（`src/builtins/ext_task.rs` の
`create_task`）は `ctx / priority / status / reason / timeslice / name / result / wakeup_tick /
join / queue / instructions` で、**親は持っていない**。`Task.new` は `task_new` →
`create_task` の 2 段で、親を足すなら `TaskData` に 1 フィールドと、`Task#parent` のような
読み出しと、`Task.current` が作る "main" ラッパの扱いが要る。今回の 3 つはどれも rubevy の側の話で、
VM を直せば rubevy は VM の次の版を待つことになる。指示どおり「明らかな 1 フィールド」ではないと見て、
VM は触らず rubevy の `src/prelude.rb` で写すことにした。

`Rubevy.adopt(task)` も考えた。`Task.new` を書き換えないので驚きが少ないが、
**ゲームが呼ばなければ何も起きない**——今 rubevy_games が手で ivar を写しているのと同じ手間を、
名前を変えて残すだけになる。穴は「書き忘れると静かに壊れる」ことなので、既定で正しい方を選んだ。

写す場所は `Task.new` の中。ここだけが「どのタスクが作ったか」を知っている
（`Task.current` が、まさに `Task.new` を呼んでいるタスク）。あとから親をたどる形（`@rubevy_parent`
を置いて `current_entity` が上へ歩く）も書けるが、ivar を 1 つ増やして読むたびに歩くことになる。
エンティティはスクリプトの生涯を通じて変わらないので、作るときに 1 回写せば同じ答えになる。
写した子がさらに `Task.new` すれば孫にも伝わる。

```ruby
class Task
  class << self
    alias __rubevy_plain_new new
    def new(*args, **kw, &block)
      task = __rubevy_plain_new(*args, **kw, &block)
      parent = Task.current
      entity = parent && parent.instance_variable_get(:@rubevy_entity)
      task.instance_variable_set(:@rubevy_entity, entity) unless entity.nil?
      task
    end
  end
end
```

**`*args, **kw` でつまずいた。** 最初は `def new(*args, &block)` と書いて
`Task.new(name: "child")` が `wrong number of arguments (given 1, expected 0)` で落ちた。
`task_new`（`ext_task.rs`）はキーワードを `kw_of`（`vm.pending_kw` が引数列の最後と同一か）で見分けるので、
`*args` に畳んでから渡すと**位置引数 1 個**になる。`**kw` で受けて `**kw` で渡すと、
キーワードが空のときは引数が 0 個のまま届く（`native_call_args` が空の Hash を足さない）。
sabiruby の `docs/worklog/2026-09-16-leftovers.md` の項目 1 が
「引数の詰め方は表現の違いにすぎない、という思い込みが 2 回破れた」と書いているのと同じ場所で、
ホストの prelude を書く側からも同じものが見えた。

素の sabiruby（`target/release/sabiruby`）で先に確かめてから rubevy に入れた:
`Task.new(name: "child")` と `Task.new` の両方が通り、`Task` オブジェクトに
`instance_variable_set` / `_get` が効くこと、`Task.current` が根のコンテキストでも nil を返さない
（"main" のラッパを作る）こと。ラッパには `@rubevy_entity` が無いので、ホスト側から作られる道では何も写らない。

テストは `tests/child_task.rs` に 3 本。子タスクが `Rubevy.ask` でエンティティ付きのリクエストを出せること
（`Rubevy.entity` も答えること）、子タスクが `Rubevy.subscribe` できて **そのエンティティ宛の** publish が
届くこと、そして `@rubevy_entity` を自分で書けば別のエンティティとして振る舞えること
（rubevy_games が手で書いていた道は残る）。前の prelude.mrb に戻すと最初の 2 本が落ち、3 本目だけ通る。

## 3. 購読が終わるときに、待っているタスクを終わらせる

reflex の worklog の最後の節が測っていたもの。倒れたロボットの `ScriptTask` が外されると
rubevy はそのタスクを `terminate` するが、`stop_task`（`ext_task.rs:398`）は
コンテキストの `ci` と `stack` を clear して `Terminated` にするだけで**巻き戻さない**ので `ensure` は走らない。
そして `Task.new` で作った子タスクは rubevy が知らないので terminate すらされず、
誰も publish しないキューの上で `WAITING` のまま残る。`Task.list` の出力に
`Scout-hit:WAITING` が 3 つ並んでいるのがそれで、ロボット 1 体につきコンテキストが 1 本、VM が生きている間ずっと。

### `Task::Queue` に何があるか

`close` はある（`ext_task.rs:854`）。`@closed` を立てて `wake_queue_waiters(vm, o, true)` で
**parkしている全部を起こす**。起きた側は Ruby の `pop` のループが `__pop_try` を呼び直し、
`queue_pop_try` は閉じたキューで `Ok(Value::Nil)` を返す（`ext_task.rs:888`）。
つまり **close は例外ではなく nil**。本家の mruby-task もそうで、
`tests/mrbtest/src/gem_queue.rb:140` の「blocking pop returns nil when queue is closed」がそれを固定している。

ここが今回の判断のしどころだった。nil のままだと、待っていたタスクは起きるが

```ruby
loop { $seen << hits.pop }
```

は **park しなくなっただけで回り続ける**。閉じたキューの `pop` は即座に nil を返すので、
寝ていた 1 本のリークが、毎フレーム命令予算を食う 1 本のリークに変わる。`ensure` も走らない。
`while (ev = q.pop)` と書いてあれば終わるが、それはスクリプトの書き方次第で、
「書き忘れると静かに壊れる」ことこそが直そうとしている穴だった。

VM に「起こして例外を上げる」入り口は無い（`terminate` は巻き戻さない、`Task::Error` は
push と非ブロッキング pop が上げるもので、待っている側には届かない）。指示が挙げていた
「prelude が分かる番兵を push する」も考えたが、番兵を見て raise するには結局 `pop` の側に
1 枚かぶせることになる。**かぶせるなら番兵は要らない**——close された上での nil が、
そのままその番兵になる。

### 決めたこと

`Rubevy.subscribe` が返すキューだけに、`Rubevy::Subscription` を `extend` する（クラスではなく
**そのオブジェクト 1 つ**に。`Rubevy.ask` のキューは今までどおり素の `Task::Queue`）。

```ruby
module Subscription
  def pop(*args)
    value = super
    raise Unsubscribed, "the subscription ended" if value.nil? && closed?
    value
  end
  alias shift pop
  alias deq pop
end
```

`extend` は Rust 側の `subscribe` ネイティブでやっている（`vm.const_get(m, :Subscription)` →
`funcall(queue, :extend, …)`）。prelude で `Rubevy.subscribe` に別名を張って包む形でも書けるが、
そうすると「購読を作る場所」が Rust と Ruby の 2 か所になる。prelude は名前を定義するだけにした。

`ScriptWorld::unsubscribe` は、キューを手放す前に `close` を送る。ここは
`ScriptTask` が外されたとき（＝エンティティが despawn された、スクリプトが差し替えられた）と、
スクリプトのタスクが自分の終わりまで走ったときの 2 か所から呼ばれる。`close` はネイティブ 1 本で、
スケジューラを回さない（起こすだけ）ので、`DeferredWorld` のフック（`stop_removed_task`）から呼んでも
再入は無い。

閉じたキューに**残っていた分は先に pop される**（`queue_pop_try` は items を先に見る）ので、
遅れていたスクリプトは取りこぼしを読んでから終わりを知る。テストの 2 本目がそれ。

### 確かめ方

`target/release/sabiruby` で先に素の Ruby で確かめた（`extend` した `pop` からの `super` が
`Task::Queue#pop` に届くか、`alias` した `shift` / `deq` からの `super` も同じか、
`ensure` が走るか、拾われなかった例外がタスクの結果になってプログラムを落とさないか）。全部そのとおりだった。

rubevy 側のテストは `tests/events.rs` に 2 本:

* `a_task_waiting_on_a_subscription_ends_when_the_script_does`:
  `Task.new` の中で `loop { hits.pop }`、`ensure` で `Rubevy.ask("reflex_ensure")`（**pop はしない**。
  巻き戻している最中に park させない）。despawn するまで ensure は走らず、despawn の後に走る。
* `a_script_can_rescue_the_end_of_its_subscription`: `rescue Rubevy::Unsubscribed` で
  自分の終わり方を決められること、閉じる前に積まれていた 1 件を読んでから終わること。

`close` の送信だけを外して同じテストを回すと 2 本とも落ちる（起きないので ensure も rescue も走らない）。

## 4. 文書と、rubevy_games 側に戻せること

`docs/host-api.md` の Events の節に 2 つ書いた。`Task.new` のタスクが作った側のエンティティを持つこと
（ivar の名前と、別のエンティティとして振る舞わせたいときは自分で書けること）と、
購読が終わるときにキューが閉じて `Rubevy::Unsubscribed` が上がること（`rescue` / `ensure` の形、
残っていた分が先に読めること、`Rubevy.ask` のキューは素の `Task::Queue` のままであること）。

`src/lib.rs` の `subscribe` ネイティブのコメントと、エンティティが無いときの文言も直した。
前は「a Task.new task has no entity」と言っていたが、いまそれは正しくない。
残るのは本当にエンティティを持たないタスク（`Script` の外でホストが spawn した、ivar を消した）だけなので、
`subscribe from a task that has an entity (@rubevy_entity)` にした。

`docs/plans/ecs-bridge-plan.md` に「続き」の表（続 1〜3、コミット付き）、`docs/README.md` に worklog の行。

**rubevy_games 側で外せるもの。** reflex の worklog が「本当は rubevy がやるべき」と書いていた 2 つは、
これで rubevy が引き受けた。

* `prelude.rb:248` と `:268` の `@rubevy_entity` の手写しは**外せる**。`Task.new` が写すようになった。
* 倒れた機体の `Scout-hit` が `WAITING` のまま残る件は**直った**。`ScriptTask` が外れると
  rubevy が購読を閉じ、`hits.pop` が `Rubevy::Unsubscribed` を上げるので、
  reflex のタスクは `ensure` を通って終わる。`docs/sabiruby-battle.md` の「まだやっていないこと」から外せる。
  ただしゲーム側の reflex のループが `loop { hits.pop }` のままだと、例外はタスクの結果になるだけで
  ログには出ない。終わりを見せたいなら `rescue Rubevy::Unsubscribed` を書く。
* `REFLEX_SLOTS`（`define_method("__reflex_#{slot}")` と `case` で呼ぶ仕掛け）は**外せない**。
  あれはエンティティの話ではなく、`instance_exec` が VM の入れ子の実行ループになって
  その中で `pop` が park できないという別の理由でそうなっている。今回の 3 つは何も変えていない。

## 確認

* `cargo test --workspace`: 41 通過（0 失敗）。内訳は `ask_value` 4、`child_task` 3（新規）、
  `components` 6、`entity` 3、`events` 10（+2）、`futures` 2、`proxy` 4、`replace` 3、doctest 6。
* `cargo build --examples` 通る。`cargo doc --no-deps` 警告 0。
* `cargo clippy --all-targets` は 2 件警告が出るが、どちらも触っていない場所
  （`drain_commands` の `SetPosition` の入れ子の `if let`、`examples/headless.rs` の Query の型）で、
  この作業の前からあるもの。範囲外なので直していない。
* prelude の `.mrb` は `docker run kishima/mruby:4.1.0-rc mrbc`（`tools/compile_scripts.sh` と同じ）で
  作り直した。`assets/scripts/*.mrb` は触っていない。
