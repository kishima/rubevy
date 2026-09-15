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
