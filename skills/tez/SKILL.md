---
name: tez
description: Use when a PHP or Laravel project should be served, set up, started, stopped or inspected through tez, the native Laravel environment (like Laravel Herd) that runs PHP, Node.js, PostgreSQL, Redis, Horizon, Reverb and Vite without Docker or Sail. Also use when the user says "run this with tez", "tez up", "turn it off", "open it in the browser", "why isn't my site loading", or asks for a project's logs, database, PHP or Node.js version, or a public share link.
---

# Using tez from a project

One tez daemon serves every project on the machine. A command run from one project can stop all the
others, so always use the narrowest command that does the job.

`tez help` is the source of truth for the installed version; if it disagrees with this file, follow
`tez help`. All commands below run in the project's root folder unless noted.

## What each command touches

| Goal | Command | Scope |
|---|---|---|
| Start this project's Horizon/queue, Reverb, Vite, scheduler | `tez up` | This project only. Remembered: they come back whenever tez starts, until `tez down`. Starts the daemon if it is not running. |
| Turn this project off | `tez down` | This project's processes only. The site stays served; other projects and services keep running. |
| Restart one process | `tez restart <site>/<process>` (e.g. `tez restart acme-shop/horizon`) | That process only. |
| Stop serving the site (files stay) | `tez unlink` (linked site) or `tez site rm <name>` | This site. `site rm --files`/`--database` delete things: only when the user asks for exactly that. |
| Stop or restart a shared service | `tez service stop\|restart postgres` | Every project that uses it. Ask first. |
| Stop or restart tez itself | `tez stop`, `tez restart` (no argument) | Every site, process and service on the machine. Only when the user asks to stop tez itself, never for "turn this project off". |

"Turn it off" for one project means `tez down`. Nothing else.

## Set up a project, in this order

1. `tez status` (is the daemon running, what is served) and `tez sites` (is this folder already a site).
2. Make it a site. A folder inside a parked folder (`~/Sites` by default) already is one. Otherwise run
   `tez link` (site name = folder name) or `tez link <name>`. Every other project command fails with
   "is not a site" until this is done.
3. `tez setup`. It does, in one go: installs the site's PHP version if tez lacks it; `composer install`;
   `.env` from `.env.example` if missing; `APP_KEY`;
   adopt (points `.env` at tez's PostgreSQL, Redis, mail, Meilisearch and S3, creates the database and
   role, keeps the original once as `.env.sail`); `npm install` on the project's Node.js;
   `php artisan migrate --force`; `storage:link`; caches. Do not hand-run these steps instead, and do
   not make your own `.env` backup.
   - If `.env` points at a shared or remote database, ask before `tez setup`: it rewrites `.env` and
     runs `migrate --force`. `tez adopt` alone does only the `.env` and database part.
4. `tez procs`: what `tez up` will run, and why each was picked.
5. `tez up`.
6. Give the user `https://<site>.localhost`. It always resolves. `https://<site>.test` works only after
   the hosts entry exists (`tez trust` or `tez hosts`, one admin prompt the user must approve).
   Until `tez trust` has run, browsers warn about the certificate and curl needs `-k`.

Horizon runs natively on Windows: tez loads a compatibility layer for `artisan horizon*`. Keep Horizon;
do not swap it for `queue:work`.

## Day-to-day

- **PHP, Composer, Node.js:** `php`, `composer`, `node`, `npm`, `npx`, `pnpm` and `yarn` on PATH are tez
  shims that pick the version for the current folder. `tez which` shows the PHP for this folder and
  `tez node which` the Node.js. Pin this site's PHP with `tez isolate 8.3` (`tez unisolate` undoes it).
  `tez use 8.4` changes the default for every site: ask first.
- **Per-project settings** go in `tez.json` in the project root, for example
  `{ "php": "8.3", "processes": { "horizon": true, "vite": false, "schedule": true } }`.
- **Logs:** `tez logs <site>/<process> -n 100` (e.g. `acme-shop/horizon`, `acme-shop/vite`),
  `tez logs postgres`, `tez logs web`, `tez logs daemon`. `-f` follows forever, so never use it in a
  foreground command; use `-n` and repeat, or run `-f` in the background. The app's own log is
  `storage/logs/laravel.log`.
- **State as JSON:** `tez status --json`. The dashboard and API are at `http://127.0.0.1:7979`; reads
  need nothing, but changes need an `X-Tez: cli` header, so prefer the CLI.
- **Databases:** `tez db list`, `tez db create <name>`, `tez db import <file> <db>`,
  `tez db pull <container> <db>` (copy from a Docker container). Services: `tez services`.
- **Share publicly:** `tez share` gives a public HTTPS address anyone with the link can open. Ask the user
  first; `tez share --allow you@team.com` limits it to those emails. Stop with `tez share stop <site>`.

## Site not loading

1. `tez status`: is `web` running, and on which ports? If port 80 is taken, tez serves on 8088/8443.
2. `tez logs web -n 50`, then `tez logs <site>/<process> -n 50`, then `storage/logs/laravel.log`.
3. `.test` does not resolve: use `.localhost`, or ask the user to run `tez trust`.
4. `tez doctor` lists what slows PHP down; `tez doctor --fix` needs an admin prompt, so leave it to the user.

## Red flags: stop and re-read the scope table

- About to run `tez stop`, `tez restart` or `tez service stop` to turn off one project.
- About to run `composer install`, `key:generate` or `migrate` by hand on a project tez has not set up.
- About to run `tez adopt` or `tez setup` before `tez link`.
- About to run `tez logs ... -f` in a command you wait on.
- About to run `tez trust`, `tez hosts`, `tez doctor --fix`, `tez share` or `tez site rm --files`
  without the user asking for it.
