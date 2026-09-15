# rubevy documents

The layout every repository of the organization uses is described in
[sabiruby/.github/CONTRIBUTING.md](https://github.com/sabiruby/.github/blob/main/CONTRIBUTING.md).
This repository is small, so its documents sit flat here; the worklog has its own directory.

| file | language | what it is |
|---|---|---|
| [host-api.md](host-api.md) | English | what a script can say to the game and the game to a script: `Rubevy.ask`, answers, entities, `Rubevy::Proxy`, time limits, replacing a script |
| [rust-bridge.ja.md](rust-bridge.ja.md) | Japanese | how a robot's question travels through rubevy and the game and back, point by point against embedding the C mruby |
| [outlook.md](outlook.md) | English | what rubevy builds on SabiRuby, in order, with the status of each item; the honest comparison with Lua; the possibilities |
| [outlook.ja.md](outlook.ja.md) | Japanese | the possibilities and the status, in plain Japanese |
| [worklog/](worklog/) | Japanese | dated records of work: what was read, tried, decided |

The worklog, newest last:

| file | what it records |
|---|---|
| [worklog/2026-09-15-host-state-and-data.md](worklog/2026-09-15-host-state-and-data.md) | the command queue moved into the VM's host state, and entities as Data objects |
| [worklog/2026-09-15-stage3b-no-internal-access.md](worklog/2026-09-15-stage3b-no-internal-access.md) | the four places that reached into `Vm`'s fields, moved to the VM's own entry points |
| [worklog/2026-09-15-stage6bc-futures-proxy.md](worklog/2026-09-15-stage6bc-futures-proxy.md) | `answer_with` (a request answered from a future), and why the dynamic proxy of stage 6c stopped at the VM's C-function boundary |
| [worklog/2026-09-15-stage6c-proxy.md](worklog/2026-09-15-stage6c-proxy.md) | the dynamic proxy finished once the VM stopped putting a boundary around `method_missing`: what `proxy.rb` is, and what it does not say about itself |
