# humOS — vision

humOS is "the OS for a human": a collection of small, terminal-first binaries
that together let a person run their own life from the command line. Mail is
the first module; calendar, notes, tasks, finances, and coordination with
other humans will follow. The north star is **running a human on autopilot** —
enough composable automation around the things a person actually does that
much of the daily overhead takes care of itself.

## Why this exists

humOS has been restarted once already (2026-04-05, branch `humos-reborn`). The
old tree went through five platform pivots — CDK/Amplify, a node-editor,
n8n+Flowise, a Chroma stack, Alon+JSON-LD datums — each time adopting a
high-level framework, hitting its ceiling, tearing it down, going lower.
Infrastructure kept racing ahead of a product that never stabilized.

The reborn version is a course correction: start from a concrete utility,
build from first principles, one real need at a time. No speculative
infrastructure. Every module must earn its place by solving a need the user
actually has.

## Core principles

### 1. `~/.humOS/` is a file-based DB, not a config store

The runtime state of humOS lives in `~/.humOS/`, and that directory is treated
as a file-backed database — not as a bag of config files. Every "thing" is
represented by its own small, uncomplicated file, or a directory containing a
few small files.

- An entity (an account, a note, a task, a contact) is a **directory**, not an
  entry in an array.
- Small facts are small files. `~/.humOS/mail/gmail/address` contains a single
  line, `felix@gmail.com`, with no parser on top.
- "List all X" = `ls ~/.humOS/X/`. "Read X" = `cat`. "Write X" = a one-shot
  write.
- There is **no top-level `config.toml`** holding arrays of things. The
  filesystem layout IS the schema.

This means any humOS binary can be understood without learning a proprietary
config format. `cat`, `ls`, and `echo >` are the query and mutation language.

### 2. humOS is a collection of small binaries

humOS is not a single `humos` CLI with subcommands. It is a suite of small,
standalone, composable binaries — one per module.

```
humos-mail     reads mail from ~/.humOS/mail/
humos-fetch    pulls mail in via native IMAP, per-account
humos-send     (future) sends mail via native SMTP
humos-web      localhost browser UI for normies
humos-cal      (future) owns ~/.humOS/cal/
humos-notes    (future) owns ~/.humOS/notes/
...
```

Each binary only knows its own directory in `~/.humOS/`. Binaries compose via
pipes and shell today, and will be driven by a higher-level autopilot later.
The feel should be closer to coreutils than to Emacs.

### 3. Own the protocols a normie needs; delegate the rest

humOS speaks IMAP and (soon) SMTP natively via Rust crates (`imap`,
`native-tls`, eventually `lettre`). This keeps the install zero-dep: a normie
runs `humos-web`, pastes an app password, clicks Sync, and sees mail. No
`brew install isync`, no `.mbsyncrc`, no external tools to install.

Passwords are stored in the OS keyring via the `keyring` crate (macOS
Keychain, Linux libsecret, Windows Credential Manager). A `password.cmd`
file is a power-user override for plugging in `1password-cli`, `pass`, etc.

For coordination with other humans — *piping data through a human as if
they were a Unix command* — the sibling project
[`tao`](https://github.com/Dorky-Robot/tao) owns the primitive: it blocks
a pipeline on a human's reply, resumes when the reply arrives, and composes
the reply back into the pipeline. humOS does not reimplement that interrupt
mechanism. tao is one primitive within the broader "coordination with
humans" module area; it is not the whole of it.

### 4. CLI-first

Every module is usable from a shell. TUIs and GUIs are not forbidden, but they
are only added if the workflow specifically demands them. The default
assumption is that the user is at a terminal prompt.

### 5. Need-by-need, no speculative infra

Before a module, a crate, a config layer, or an abstraction exists, there must
be a concrete need it serves right now. Features are not added "because we'll
probably want them." When in doubt, defer.

### 6. Test-first

humOS is developed test-first. A failing test exists before the implementation
code it covers. Pure functions are preferred where possible (paths and IO as
parameters, not hardcoded), so testing does not require mocking filesystem
globals.

## Worked example: mail

```
~/.humOS/mail/
  gmail/
    address        ← "felix@gmail.com"
    imap.host      ← "imap.gmail.com"
    imap.port      ← "993"
    smtp.host      ← "smtp.gmail.com"
    smtp.port      ← "587"
    password.cmd   ← "security find-generic-password -s humos-gmail -w"
    INBOX/
      new/ cur/ tmp/
  icloud/
    address        ← "felix@icloud.com"
    ...
```

- `ls ~/.humOS/mail/` is the list of accounts.
- Adding a new account: `humos-web` → "Add account" form (or `mkdir` + `echo`
  for power users). Password goes into the OS keyring; `password.cmd` is an
  optional override.
- `humos-fetch gmail` reads `~/.humOS/mail/gmail/imap.*`, retrieves the
  password from the keyring, connects via native IMAP, and writes new mail
  into `~/.humOS/mail/gmail/INBOX/new/`.
- `humos-mail` walks `~/.humOS/mail/*/INBOX/{new,cur}` and shows unread counts
  and summaries.
- `humos-web` serves a localhost browser UI with add-account, sync, and
  archive.

No god file. The file-based DB is the source of truth; passwords live in the
OS keyring, not on disk.

## Modules

Each module is a standalone binary (or set of binaries) that owns a
subdirectory of `~/.humOS/`. Modules compose via shell and file-system
contracts.

**In progress:**

- **Mail** (`humos-mail`, `humos-fetch`, `humos-web`) — multi-account Maildir
  under `~/.humOS/mail/`. `humos-fetch` pulls mail via native IMAP.
  `humos-web` provides a localhost browser UI with account setup, sync, and
  archive. Sending via native SMTP is upcoming.

**Planned, humOS-native:**

- **Finances** (`humos-fin`) — personal ledger, transactions, budgets,
  `~/.humOS/fin/`. Likely interoperable with plain-text accounting formats
  (ledger / hledger / beancount) rather than inventing a new format.
- **Calendar** (`humos-cal`) — read/write `~/.humOS/cal/`, ical-compatible.
- **Tasks** (`humos-tasks`) — `~/.humOS/tasks/`.
- **Autopilot** — the layer that stitches modules together into recurring
  routines and responses. humOS is the "company"; agents (via `abot`) are
  the workers; rooms (via `kubo`) are where they work; the human approves
  results (via `tao`). Autopilot watches `~/.humOS/` for signals (new
  mail, new tasks, calendar events), dispatches agents to handle them,
  and gates the human for approval before external actions. See
  [ecosystem.md](ecosystem.md) for the full architecture.

**Delegated to sibling Dorky-Robot projects:**

- **Notes / journal — [`tala`](https://github.com/Dorky-Robot/tala).** tala
  is a self-hostable Elixir notes service where every note is its own git
  repo. It owns markdown storage, per-note version history, and both live
  (websocket) and async (cross-instance) collaboration. humOS does not
  reimplement any of that — if a module needs to reference or record a
  note, it talks to tala's API or shells out to tala's CLI.
- **Pipe-to-a-human — [`tao`](https://github.com/Dorky-Robot/tao).** See
  the "Coordination with other humans" section below for the full
  discussion.
- **Agent identities — [`abot`](https://github.com/Dorky-Robot/abot).**
  A headless CLI that manages AI agent identities as git repositories.
  Agents can be cloned, employed into rooms (via git worktrees),
  and integrated back (via git merge). humOS shells out to `abot` when
  it needs to dispatch work to an AI agent. See
  [ecosystem.md](ecosystem.md) for how abot, kubo, tao, and humOS
  compose.
- **Container rooms — [`kubo`](https://github.com/Dorky-Robot/kubo).**
  Isolated Docker containers where agents work. humOS shells out to
  `kubo` to create rooms and mount agent working directories into them.

### Coordination with other humans

Running a human on autopilot means talking to *other* humans too — asking
questions, delegating decisions, sharing context, tracking ongoing threads.
This is a whole module area, not a single binary. humOS will grow into it
one concrete need at a time.

**The first primitive: pipe-to-a-human, via tao.** The sibling Dorky-Robot
project [`tao`](https://github.com/Dorky-Robot/tao) provides exactly one
thing, but it's load-bearing: a Unix command that *interrupts* a human as
if they were a regular program. `tao approve-pr developer felix` reads
stdin, sends it to Felix via email, blocks until he replies, and writes
the reply to stdout. The pipeline treats Felix as a command that happens
to take longer than `grep`. State lives in `~/.tao/` following the same
file-based-DB philosophy as humOS.

humOS does not reimplement this primitive. When a module needs to pipe
data through a human, it shells out to `tao` the same way it shells out
to `mbsync` or `msmtp`. `~/.tao/` and `~/.humOS/` are peer file-based DBs
today; convergence is possible later, not prescribed now.

**tao is one primitive, not the whole module.** Other coordination
capabilities — shared state with other humans, long-lived threads,
scheduling across people, contact/relationship directories — will be added
as humOS hits concrete needs for them. Each is its own primitive, its own
binary, its own `~/.humOS/` subdirectory.

### Candidates for resurrection from the old tree

Resurrect only when forced by a specific need:

- **datum** (content-addressable fact store) — if we need versioning or
  cross-module provenance.
- **WebAuthn-signing** — if we need cryptographic identity for any of the
  remote-facing modules.
