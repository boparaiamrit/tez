# tez

**Fast, native Laravel development on Windows. No WSL, no Docker, no Sail.**

tez (Punjabi and Hindi for "fast") is one Rust program that installs and runs everything a Laravel
project needs, natively on Windows: every PHP version, a web server with local HTTPS, PostgreSQL with
TimescaleDB, MySQL, MariaDB, MongoDB, Redis, Meilisearch, S3 storage and a mail catcher. It also
runs each project's Horizon, Reverb, Vite and scheduler. A desktop app with a tray icon lets you
change every setting, and so does a web dashboard and the `tez` command line.

It covers Laravel Herd and Herd Pro's feature set, and adds Horizon running natively on Windows.

## Why

On this machine, projects ran in WSL with Sail. That meant a 16 GB virtual machine, slow file access
across the Windows/Linux boundary, and memory running out when several agents worked at once. Herd
for Windows avoids WSL, but every request starts Laravel from scratch through `php-cgi`, and
Microsoft Defender scans every file it touches.

tez attacks the costs one by one:

| Cost on Windows | What tez does |
| --- | --- |
| Every request re-reads thousands of files | OPcache on for web **and CLI**, a 16 MB realpath cache, an on-disk opcode cache for cold starts |
| Filament's Blade Icons lists thousands of SVGs per request | `icons:cache` runs automatically (`tez adopt` and `tez up`): a Filament page went from ~800 ms to ~100 ms |
| `artisan`, Horizon and queue workers compile everything per process | CLI OPcache: `artisan route:list` 3.6 s → 0.8 s, each worker 88 MB → 12 MB |
| Defender scans every PHP file read | `tez doctor --fix` excludes the tez folder and your site folders (one admin prompt) |
| WSL's memory and its file bridge | Projects live on NTFS and run natively; WSL can be shut down |
| A full SSD | tez refuses to download or copy when the disk would drop under 2 GB free |

Measured on this machine (warm, HTTPS, with Defender still scanning): Acme's login page in about
110 ms on PHP 8.4, Example's admin login in 76–83 ms on PHP 8.1.

## Install

Download the build for your system from the
[Releases page](https://github.com/boparaiamrit/tez/releases): a zip for Windows, a `.dmg` for
macOS on Apple silicon, a `.tar.gz` for Linux. Each release is built, smoke-tested and packaged by
GitHub Actions on its own operating system. The macOS app is not notarised yet; the first time,
right-click it and choose Open.

Or build it yourself:

```powershell
cargo build --release --workspace     # tez and tez-app (Windows needs the VS 2022 C++ tools)
.\target\release\tez.exe install --all
```

`install --all` downloads PHP 7.4 to 8.6, the Caddy web server, PostgreSQL 17 with TimescaleDB, Redis,
Mailpit, Meilisearch and RustFS into `%USERPROFILE%\.tez`, and adds `%USERPROFILE%\.tez\bin` to your
PATH (open a new terminal afterwards). Without `--all` it installs only the default PHP plus
PostgreSQL, Redis and Mailpit.

Then, once:

```powershell
tez start          # everything in the background, plus the tray icon
tez trust          # trust local HTTPS and add hosts entries (one admin prompt)
tez doctor --fix   # Defender exclusions for tez and your sites (one admin prompt)
```

## Daily use

```powershell
tez open                         # the desktop app (or click the tray icon)
tez status                       # processes, memory, sites
cd ~\Sites\myapp; tez up         # Horizon or queue, Reverb, Vite, scheduler for this project
tez logs myapp/horizon -f
```

Any folder in `%USERPROFILE%\Sites` is a site at `https://<folder>.test`, and always at
`https://<folder>.localhost` too (browsers resolve `.localhost` with no hosts entry).
`php`, `composer` and `exec` on your PATH pick the PHP version of the site you are in.

### Moving a project off WSL

```powershell
tez import /home/me/code/myapp           # copies it out of WSL (measures first; skips vendor,
cd ~\Sites\myapp                         #   node_modules, caches and folders over 1 GB)
tez setup                                # composer install, .env, key, database, migrations, npm install
tez up
```

`tez setup` runs `tez adopt`, which points the `.env` at tez's services (`DB_HOST=127.0.0.1`,
`REDIS_HOST`, mail, Meilisearch, S3). It keeps the original as `.env.sail`, creates the database
and role from the `.env`, and serves every `*_DOMAIN` host for multi-domain apps. For a Docker
database you want to keep, use `tez db pull <container> <db>`.

### Cloning from GitHub

The desktop app's **New site from GitHub** page (or `tez clone owner/repo`) lists the repositories
of every account signed in to the GitHub CLI and its organisations. It clones through the matching
SSH alias from `~/.ssh/config` (for example `github.com-work`) or through gh over HTTPS, then runs
`tez setup`. Any git URL works too. Tokens stay in gh's store; tez never keeps them.

## The desktop app

`tez-app` is a native window built with [GPUI Kit](https://gpui-kit.com), plus a tray icon (the
menu bar on macOS, an AppIndicator on Linux). It draws its own title bar on Windows and macOS: File,
Edit, View and Help menus, back and forward, and the window buttons. Closing the window leaves tez
in the tray.

The window has an icon rail (Home, Sites, Services, PHP, Debug), a list for the current section and
an inset panel. Each page answers one question first:

- **Home: is my machine OK, and are my sites fast?** Problems that need you, each with its fix;
  memory over 15 minutes, an hour or a day, split into PHP pools, site processes, services and the
  web server; the whole machine (tez, WSL, other apps, free) and the disk; a card per site with
  requests per minute, p95 response time and server errors; a card per service.
- **A site: is it fast and healthy?** p50 and p95 response times, the slowest routes, the latest
  server errors and ERROR entries in laravel.log, all read from the web server's own log with no code
  in your app; queue throughput and the slowest jobs, read from Horizon's output; Horizon, Reverb,
  Vite and the scheduler as cards with their memory, CPU and restarts; a box to run artisan,
  composer or npm in the site. Tabs for the environment (secrets masked), domains, config files and
  logs.
- **Services, PHP, Dumps, Mail, Logs** as before, and **Settings** in a sheet.

Things you control are cards: the switch top-right turns the whole thing on or off, status sits
under the name, what it costs is in the strip below, and actions are in the footer.

Typed changes save themselves shortly after you stop typing. Secrets (APP_KEY, passwords, API keys,
the Meilisearch master key) are masked, have a show toggle and their own Save button, and say
where they are kept. The same settings are in the web dashboard at `http://127.0.0.1:7979`, which
also has logs, dumps and a config file editor.

## What runs

| Part | Source | Notes |
| --- | --- | --- |
| PHP 7.4–8.6 | windows.php.net official builds, SHA-256 checked | redis, mongodb and xdebug from PECL; `tez ext install <any>` for the rest |
| Web server | Caddy 2.11 | local HTTPS CA, HTTP/2, compression, websockets; runs no PHP itself, every site runs on the official PHP builds |
| PHP pools | `php-cgi` per version | 6 workers each, restarted if they exit |
| PostgreSQL 14–18 | EnterpriseDB builds | TimescaleDB (several versions side by side), PostGIS, pgvector, psql, pg_dump |
| MySQL 8.0/8.4, MariaDB 10.11–12.3 | official zips | root, no password, 127.0.0.1 |
| MongoDB 6.0–8.2 | fastdl.mongodb.org | |
| Redis 8 | redis-windows builds | |
| Meilisearch, Mailpit | official releases | Mailpit's inbox at http://127.0.0.1:8025 |
| S3 | RustFS | MinIO stopped publishing binaries in 2025; key `tez`, secret `tez-secret` |

Several versions of a service can run at once: `tez service install postgres 16` adds `postgres@16`
on its own port beside the default PostgreSQL 17.

## macOS and Linux
nLinux needs glibc 2.36 or newer (Ubuntu 24.04, Debian 12, Fedora 37 and later): the PostgreSQL builds tez downloads are linked against it.

The same `tez` binary runs on macOS (Apple silicon and Intel) and Linux (x86_64 and arm64). What
differs from Windows:

| Part | macOS / Linux source | Notes |
| --- | --- | --- |
| PHP 8.0–8.5 | static-php-cli's prebuilt "bulk" builds (dl.static-php.dev) | One `php` and `php-fpm` per version with ~60 extensions built in (opcache, intl, redis, pcntl, posix, pdo_pgsql, pdo_mysql, pdo_sqlite, gd, imagick, swoole). Extensions cannot be switched or added per version; PECL add-ons, Xdebug and PHP 7.4 are Windows-only for now |
| PHP 8.6 | not published yet as a static build | Sites that need 8.6 run on Windows for now |
| PHP pools | `php-fpm`, one master per version | `php_workers` children each |
| Web server | Caddy 2.11 | The same server on every platform |
| PostgreSQL 14–18 | theseus-rs/postgresql-binaries | TimescaleDB, PostGIS and pgvector are Windows-only for now |
| Redis | compiled from the source release with `make` | Needs the Xcode Command Line Tools, or `build-essential` on Linux |
| MariaDB | official bintar | Linux x86_64 only |
| MongoDB, MySQL, Mailpit, Meilisearch, RustFS | official builds per OS | MongoDB on Linux: Ubuntu, Debian, RHEL, Amazon Linux. RustFS: no Intel Mac build |

Processes run in their own process groups instead of Job Objects; the daemon ends them all on
`tez stop`, SIGTERM or SIGINT, and a new daemon ends any a crashed one left behind. `tez trust`
and `tez hosts` use `sudo` (the CA goes into the System keychain on macOS, or
`update-ca-certificates` / `update-ca-trust` on Linux). Start at sign-in is a LaunchAgent on macOS
and an XDG autostart entry on Linux. `tez install` adds `~/.tez/bin` to PATH in `~/.zshrc`,
`~/.bashrc` or `~/.profile`. On Linux, ports 80 and 443 need root, so tez serves on 8088 and 8443.

For CI and scripts: `tez install --php 8.4 --services redis,postgres` (or `--services none`)
installs without prompts, `tez start` works without a display, and `tez status --json` prints the
daemon's state.

## Horizon on Windows

Windows PHP has no `pcntl` or `posix` extension, and Horizon needs both. tez loads a small PHP
layer (`lib/php/horizon-windows.php`) into `artisan horizon*` processes only. That layer:

- defines the `SIG*` constants and the `pcntl_*` / `posix_*` functions Horizon calls
- delivers signals between Horizon's processes through mailbox files, picked up by the master and
  supervisors on their `sleep(1)` loop, so `horizon:terminate`, `pause` and `continue` work
- checks process liveness through kernel32 (FFI)

The command shims also make Horizon's `exec @php ...` command lines work in cmd.exe. Everything
Horizon writes to Redis is Horizon's own code, so the `/horizon` dashboard works as on Linux.
Composer skips the `ext-pcntl`/`ext-posix` platform check through the `composer` shim.

## Security

- Every service and the dashboard listen on 127.0.0.1 only.
- The API accepts only its own host names (no DNS rebinding) and refuses changes without the
  `X-Tez` header, which web pages on other sites cannot send.
- Dumps render in sandboxed frames with scripts off.
- Local PostgreSQL trusts connections from 127.0.0.1, so project passwords are not checked
  locally; they are kept in `.env` for parity with production.
- Every process tez starts runs in a Job Object, so nothing outlives tez.
- `tez trust` installs tez's own certificate authority into Windows' trusted roots. It never
  leaves this computer.

## Layout

```text
src/            the tez binary: CLI, daemon, API, PHP, services, sites, web server, Horizon support
app/            tez-app: the GPUI Kit desktop app and tray icon
assets/         the web dashboard and the PHP files tez loads into PHP
%USERPROFILE%\.tez\
  bin/          tez.exe, tez-app.exe, php.exe, composer.exe, exec.exe
  php/<x.y>/    PHP versions          services/   service binaries
  data/         databases and stores  config/     tez.json, Caddyfile, certificates
  logs/         one log per process   cache/      downloads
```

## Not done yet

- Typesense has no Windows build, so it is not offered (Herd for Windows lacks it too).
- Pausing Horizon from the dashboard restarts supervisors rather than pausing them in place.
- Port 80 falls back to 8088 while another program (here, nginx inside WSL) holds it.
