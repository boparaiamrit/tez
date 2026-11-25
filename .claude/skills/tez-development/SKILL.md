---
name: tez-development
description: Use when changing tez's own code in this repository (the CLI, daemon, HTTP API, PHP, Node.js and service installers, web server config, tunnels, src/sys), when adding a command, service or managed process, when building or testing a change or proving it works end to end, or when deploying a new build to the tez running on this machine.
---

# Working on tez

tez is usually installed on the machine you are working on, running from `~/.tez` and serving the
user's real projects. The repo and the live install share ports and files. The core skill here is
changing tez without touching the running one.

## Map

| Path | Holds |
|---|---|
| `src/main.rs` | multi-call entry: when run as `php`, `composer`, `exec`, `node`, `npm`... it is a shim (`shim.rs`, `node.rs`); otherwise the CLI |
| `src/cli.rs` | every command. `help()` is the user-facing command list: update it, and the README, with any new command |
| `src/daemon.rs` | the supervisor: `plan()` from config, then `reconcile`/`tick`; process states and `backoff()` |
| `src/api.rs`, `src/manage.rs` | HTTP API on 127.0.0.1:7979 and the logic behind it |
| `src/services.rs`, `catalog.rs`, `php.rs`, `node.rs` | installers, versions, catalogs |
| `src/web.rs`, `nginx.rs`, `tls.rs` | Caddy or Nginx config and tez's own certificate authority |
| `src/project.rs` | a site's processes for `tez up`, and `adopt` (.env rewriting) |
| `src/sys/windows.rs`, `src/sys/unix.rs` | hand-written FFI: Job Objects, process groups, hidden spawning |
| `app/` | tez-app, the desktop app: see the tez-desktop-app skill |
| `assets/` | the web dashboard and the PHP files tez loads into PHP (Horizon on Windows) |

Dependencies are deliberately few (serde, serde_json, rcgen, time); downloads go through the OS's own
curl, tar and certutil in `fetch.rs`. Ask before adding a crate.

## Rules, each paid for by an incident

1. **Leave the live tez alone unless the user asks for a deploy.** That means no
   `tez stop|start|restart|install|trust|hosts`, no `cargo run -- start` (start copies the build into
   `~/.tez/bin`), no writes under `~/.tez`, and never `scripts/smoke.ps1` or `smoke.sh` here (they
   install into `~/.tez` and end with `tez stop`). About 20 unrequested redeploys once caused a storm
   of windows and exhausted memory. Reading `tez status`, the API and `~/.tez` files is fine.
2. **Cap build parallelism.** Use `-j 4` (`-j 2` if memory is tight), and prefer `--release`. Debug
   builds of the workspace failed with rustc mmap errors and STATUS_STACK_BUFFER_OVERRUN.
3. **No console windows.** A managed process goes through `daemon::start`, which applies
   `sys::prepare_child` and `Job::attach`. A one-off command tez runs for itself uses `.hidden()` (the
   `sys::Hidden` trait). A bare `Command::new(..).spawn()` flashes a console window on Windows, and
   dozens of them made the window storm.
4. **`State::Failed` is never retried on its own** (`next_start = None`; `backoff()` caps at 30 s).
   Failed processes that retried forever (one service restarted 57 times) fed the window storm. If a
   request would make failures retry without end, tell the user this history and offer a bounded
   alternative before changing it.
5. **Verify and make room before you download or copy.** Check SHA-256 or the GitHub asset digest, and
   call `fetch::ensure_space` (2 GB floor). A copy once filled the C: drive to 0 bytes.
6. **Listen on 127.0.0.1 only.** A route that changes state goes behind `allowed()`, the host allowlist
   plus the `X-Tez` header.
7. **The repo is public.** Code, tests, comments, commit messages and screenshots use demo names
   (hello, acmeshop, marketplace, recipes, storefront) and `C:\Users\you`. Never use one of the user's
   real project names, their username or a local path. `tez sites` on this machine shows the names to
   keep out.

## Prove a change

| Change | Proof |
|---|---|
| Any Rust | `cargo test -p tez -j 4` (49 pass and 1 is ignored as of 0.2.7 with the restart fix), with a unit test beside the code you changed |
| A command that does not start the daemon | `target/release/tez(.exe) <command>` with `TEZ_HOME` set to a scratch folder in the same shell; delete the folder afterwards. `TEZ_HOME` moves config, bin, data and logs. Its `tez.json` can change the API and web ports, but not the fixed ones below. |
| Daemon, processes, web server, services, PHP pools | Not beside the live tez: PHP pools use fixed ports (`9000 + major*100 + minor*10 + n`), and Caddy's admin `127.0.0.1:2019` and the service ports are fixed, so a second daemon collides with the live one. Push a branch, then run `gh workflow run ci.yml --ref <branch>` and `gh run watch <id> --exit-status`. That is the end-to-end smoke test on clean Windows and Linux runners. |
| macOS or Linux code | CI covers Linux on every push. macOS runs only in the release workflow: `gh workflow run release.yml --ref <branch>` builds, tests and packages all three systems and publishes nothing. |
| The desktop app | the tez-desktop-app skill |
| A release | the tez-release skill |

Say which of these ran. "Unit tests pass" is not "works end to end".

## Deploy to the live tez (only when the user asks)

Deploy once per batch, never per edit:

```powershell
cargo build --release --workspace -j 4
tez stop                                  # stops every site and service on the machine
taskkill /IM tez-app.exe /F               # Windows: the app holds its exe open
.\target\release\tez.exe start --quiet    # copies the build into ~/.tez/bin, starts daemon and tray
tez status                                # every process back to running?
```

Tell the user what restarted, and that the app window shows the new build next time it opens.

## Talking to the live daemon

Reads are fine: `tez status --json`, or `curl http://127.0.0.1:7979/api/status`. A POST needs
`-H "X-Tez: cli"`, and gets 403 without it; it changes the user's live setup, so ask first.
