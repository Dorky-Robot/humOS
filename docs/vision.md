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
humos-mail     agent-facing CLI for reading/triaging/composing mail in
               ~/.humOS/mail/; wraps himalaya and adds cross-account verbs
humos-fetch    pulls mail per-account (himalaya under the hood)
humos-send     (future) sends mail (himalaya under the hood)
humos-triage   AI-powered mail classification via local Ollama
humos-web      localhost browser UI for normies (account setup, sync, archive)
humos-index    cross-cutting search across mail and tala notes;
               parses GFM checkboxes and inline dates into todo/event views
humos-cal      (future) a view over tala notes with date frontmatter
humos-tasks    (future) a view over GFM checkboxes in tala notes
...
```

Each binary only knows its own directory in `~/.humOS/`. Read-across
binaries are the exception: `humos-index` reads across `~/.humOS/` and
`~/.tala/` but only writes to `~/.humOS/index/`; `humos-tasks` and
`humos-cal` are thin views over tala notes (via humos-index) and own
no storage of their own. Binaries compose via pipes and shell today,
and will be driven by a higher-level autopilot later. The feel should
be closer to coreutils than to Emacs.

### 3. Own the protocols a normie needs; delegate the rest

humOS delegates IMAP and SMTP to
[himalaya](https://github.com/pimalaya/himalaya), a Rust CLI email client
with JSON output and a Maildir backend. `humos-mail` wraps it: adds
cross-account verbs (himalaya operates one account at a time) and
integrates with humos-index for unified search across accounts + tala.
The file-based DB (`~/.humOS/mail/{account}/{address,imap.host,...}`)
remains the source of truth; `humos-mail` generates
`~/.config/himalaya/config.toml` from it as a derived artifact — the
god-file pattern stays outside our principles. A normie still runs
`humos-web`, pastes an app password, and clicks Sync — `brew install humos`
pulls himalaya as a dependency, so there's nothing else to install.

Passwords live in the OS keyring (macOS Keychain, Linux libsecret, Windows
Credential Manager). himalaya's `auth.cmd` maps 1:1 to the humOS
`~/.humOS/mail/{account}/password.cmd` convention, so `1password-cli`,
`pass`, etc. plug in without humOS-specific glue.

The audience for the humos-mail CLI is **agents, not humans**. Humans who
want a pretty inbox have Gmail or Apple Mail. humos-mail's job is to give
abots the full verb surface a human would have in a GUI — as pipeable,
JSON-capable shell commands. Every read verb supports `--json`, every
write verb reads stdin, no interactive prompts, IDs stable across syncs.
An abot running in a kubo with shell access calls these verbs the same
way a human would: `humos-mail list --unread --json | ...`. No new
protocol layer — the CLI is the interface. (If a future abot runtime
can't exec shell commands, an MCP or HTTP wrapper is an *additive* concern
for that runtime, not something humOS builds upfront.)

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

### 7. Prefer markdown-native conventions over invented schemas

Where a markdown spec or widely-adopted inline convention already expresses
an idea, humOS uses it rather than inventing a parallel schema or a
separate storage directory.

- Todos are GFM checkboxes (`- [ ]` / `- [x]`) inside tala notes, not rows
  in `~/.humOS/tasks/`.
- Calendar events are tala notes with date frontmatter or inline dates,
  not entries in `~/.humOS/cal/`.
- Tags and metadata use inline conventions (`@due(2026-04-20)`, `@urgent`,
  `#project-foo`) that a human reading the raw markdown still understands.

`humos-index` parses these conventions into derived views. The raw
markdown remains the source of truth, portable to any editor.

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
- `humos-fetch gmail` reads `~/.humOS/mail/gmail/imap.*`, refreshes the
  himalaya config entry for this account, and invokes himalaya to sync
  new mail into `~/.humOS/mail/gmail/INBOX/new/` (password from keyring).
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
  under `~/.humOS/mail/`, wrapping
  [himalaya](https://github.com/pimalaya/himalaya) for IMAP/SMTP and JSON
  verbs. `humos-mail` adds the cross-account layer himalaya doesn't have
  (unified inbox, search-across-accounts via humos-index) and is designed
  to be called by abots directly via shell. `humos-web` handles account
  setup, sync, and archive for normies.

**Planned, humOS-native:**

- **Finances** (`humos-fin`) — personal ledger, transactions, budgets,
  `~/.humOS/fin/`. Likely interoperable with plain-text accounting formats
  (ledger / hledger / beancount) rather than inventing a new format.
- **Calendar** (`humos-cal`) — a *view*, not a store. Reads tala notes
  with date frontmatter or inline dates via humos-index and presents an
  ical-compatible feed. No `~/.humOS/cal/` directory — the source of
  truth is the note itself.
- **Tasks** (`humos-tasks`) — a *view* over GFM checkboxes (`- [ ]` /
  `- [x]`) in tala notes, with optional inline tags like `@due(2026-04-20)`
  or `@urgent`. No `~/.humOS/tasks/` directory. A todo lives in the note
  that explains *why* it exists; the view just collects them.
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
