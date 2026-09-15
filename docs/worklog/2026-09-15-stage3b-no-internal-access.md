# 作業記録: VM の内部フィールドに触るのをやめる（段階 3b、rubevy 側）

sabiruby `docs/plans/host-bridge-plan.md` の段階 3b の後半。前回の記録
（`2026-09-15-host-state-and-data.md` 2.7 節）で「公開 API に相当するものが VM 側に無い」として
残した 4 か所を、VM に入った入口に置き換えた。VM 側の作業と、なぜその形の関数になったかは
sabiruby の `docs/worklog/2026-09-15-stage3b-host-entry-points.md` に書いた。

worktree は `rubevy-wt-host`、ブランチは `no-internal-access`。ベンチは回していない
（別の担当が同じ機械で sabiruby の計測中）。ゲーム（rubevy_games）には触っていない。

## 0. 手元の VM をどう向けるか

VM の新しい関数はまだ `github.com/sabiruby/sabiruby` の main に無いので、README の
「Building against the VM」の手順で、git 管理外の `.cargo/config.toml` に patch を置いて
sabiruby の worktree を向けた:

```toml
[patch."https://github.com/sabiruby/sabiruby"]
sabiruby = { path = "/home/kishima/book/kishima/sabiruby-wt-host" }
sabiruby-compiler = { path = "/home/kishima/book/kishima/sabiruby-wt-host/compiler" }
```

`.gitignore` に `.cargo/config.toml` があるので `git status` には出ない。コミットしていない。
`Cargo.lock` もこの patch では動かないので触っていない（本体がマージ後に sabiruby の新しい
コミットへ更新する）。

## 1. 4 か所

置き換えは機械的で、迷うところは無かった。

| 前 | 後 | 場所 |
|---|---|---|
| `let k = vm.intern("@rubevy_entity"); vm.heap.ivar_set(task, k, …)` | `vm.ivar_set(task, ENTITY_IVAR, …)` | `start_scripts` |
| `let Some(task) = vm.task.running else …; vm.heap.ivar_get(task, k)` | `vm.task_running()` + `vm.ivar_get(task, ENTITY_IVAR)` | `current_entity` |
| `let n = vm.intern("$rubevy"); vm.globals.insert(n, Slot::from(h))` | `vm.global_set("$rubevy", h)` | `set_frame_state` |
| `fn is_exception(vm, v) { … matches!(vm.heap.get(o).kind, ObjKind::Exception) … }` | `world.vm.is_exception(value)` | `drain_ended` |

`is_exception` は rubevy 側の private な関数ごと消えた（VM のものと同じ判定なので、同じものを
2 つ置く意味が無い）。`sabiruby::value::Slot` と `sabiruby::object::ObjKind` の import も消えて、
`use sabiruby::value::ObjId;`（`ScriptTask` が持つ型）だけが残った。

2 つ細かい変更を伴った:

* `"@rubevy_entity"` という文字列が 2 か所に書かれていて、どちらも VM の `intern` に渡っていた。
  書く側と読む側が一致していないと黙って `nil` になる種類の重複なので、`ENTITY_IVAR` という
  private な `const &str` にまとめた。新しい入口が `&str` を取るので、これが自然に置ける形になった
  （前は `Sym` を取るので、定数にするなら intern の結果を持ち回る必要があった）。
* `current_entity(vm: &mut Vm)` を `&Vm` に狭められた。`task_running` も `ivar_get` も `&self` で、
  以前は「名前を `Sym` にするのに `intern` が要る」という理由だけで `&mut` だった。呼び出し側は
  ネイティブの中の `&mut Vm` なので、そのまま再借用で通る。

## 2. 「0 になった」ことをどう確かめたか

grep を 2 本。1 本目は内部フィールドへの到達、2 本目はそのために要っていた import:

```
$ grep -rn "vm\.heap\|vm\.task\b\|vm\.globals\|\.heap\.\|\.globals\.\|\.task\.running" src/ tests/ examples/
$ grep -rn "Slot\|ObjKind" src/ tests/ examples/
```

どちらも 0 件。`vm.task_*` は 1 本目の `vm\.task\b` に引っかからない（`task_spawn` などは
`vm.task_` で `\b` の位置が違う）ことを、わざと `vm.task_spawn` を含む行で確かめてある。
残った内部アクセスは無い。

`src/` に残る sabiruby の型は `Value`、`Vm`、`VmError`、`ObjId`、`convert::{DataRef, This}` で、
どれも公開 API として文書化されているもの。

## 3. 確認

`cargo tree` で patch が効いていることを先に確かめた（`sabiruby v0.4.0
(/home/kishima/book/kishima/sabiruby-wt-host)`）。そのうえで:

* `cargo build`: 警告なし（18.8 秒）。
* `cargo test --workspace`: 6 passed / 0 failed（`tests/replace.rs` 3 本、`tests/entity.rs` 3 本）。
* `cargo build --examples`: `headless` と `sensor` が通る。
* `cargo doc --no-deps`: 警告なし。
* `cargo clippy --all-targets`: 警告 2 件（lib の `very complex type used`、example `headless` の
  `this if statement can be collapsed`）。**変更前と同じ 2 件**。同じ worktree で `git stash` して
  HEAD の状態に戻し、同じ patch・同じ target ディレクトリで clippy を回して数えた。増えていない。

`Cargo.lock` は触っていない。`.cargo/config.toml` の patch を当てて cargo を動かすと、lock の
`sabiruby` / `sabiruby-compiler` から `source = "git+…"` の行が消える（パスに置き換わる）ので、
コミットの前に毎回 `git checkout -- Cargo.lock` で戻している。本体がマージ後に sabiruby の
新しいコミットへ更新する。

## 4. 気づいたが直さなかったこと

`docs/rust-bridge.ja.md` の「補足: `unsafe` の数について」が「VM の crate の `unsafe` は 1 か所で、
命令のバイト値を範囲確認したうえで enum に変換する箇所（`src/opcode.rs`）」と書いているが、
段階 0（2026-09-15、sabiruby `354b6bb`）でこれは 0 になっている。段階 3b の範囲外なので直していない。
同じ文書の 5 節のベンチの数値（fib 3.5 倍、`so_lists` 15.8 倍）も段階 2・2b・2c の前のもので、
今は `docs/verification/bench.md` の方が新しい。
