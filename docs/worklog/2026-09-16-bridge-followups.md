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
