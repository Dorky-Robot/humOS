# The Dorky Robot Stack

A human has inputs (email, messages, calendar events) and needs to produce
outputs (replies, tasks completed, decisions made). In between is data
transformation — triage, prioritize, draft, review, execute, follow up.

The Dorky Robot stack models this as a **company of one**: a flat
organization of AI agents ("workers") that operate on behalf of the human.
The human is the CEO — they approve things, they don't micromanage. The
agents self-organize around signals from the human's data.

## The primitives

Four standalone CLI tools, each doing one thing:

```
humOS   the human's data (mail, calendar, tasks, finances)
abot    AI agent identities (create, clone, employ, integrate)
kubo    containerized rooms where agents work
tao     pipe-to-a-human (block a pipeline on human approval)
```

Each is installed independently. Each is useful on its own. humOS
composes all three into a personal automation system, but a developer
building something completely different could use abot + kubo + tao
without humOS.

### humOS — the data layer

Owns `~/.humOS/`. Filesystem-as-DB. Small binaries that each own a
subdirectory. No Docker required, no agents required. A person who
never wants AI can use `humos-fetch` and `humos-mail` and get value.

```
~/.humOS/
  mail/          ← humos-fetch, humos-mail, humos-triage, humos-send
  cal/           ← humos-cal (planned)
  tasks/         ← humos-tasks (planned)
  fin/           ← humos-fin (planned)
  prompts/       ← reusable LLM prompt templates
```

See [vision.md](vision.md) for full design principles.

### abot — the agent primitive

Owns `~/.abot/`. Headless CLI. An agent ("abot") is a git repository
with an identity, config, and working directory. Agents can be cloned,
employed into rooms, dismissed, and integrated back — all via git
operations (branches, worktrees, merges).

```
~/.abot/
  agents/
    alice.abot/        ← canonical git repo
      .git/
      manifest.json    ← name, version, created/updated timestamps
      config.json      ← shell, env vars, personality/instructions
      home/            ← the agent's working directory
    bob.abot/
      ...
```

Key operations:

```bash
abot create alice              # git init an agent identity
abot list                      # show all agents
abot clone alice alice-draft   # snapshot: new repo from alice's current state
abot employ alice room-name    # git worktree on branch kubo/room-name
abot dismiss alice room-name   # remove from room, keep branch history
abot integrate alice room-name # git merge kubo/room-name into alice's main
abot discard alice room-name   # git branch -D, throw away the work
abot log alice                 # git log — the agent's full history
```

abot knows nothing about Docker, sessions, or UIs. It manages git
repos and worktrees. The caller (humOS, a shell script, whatever)
composes abot with kubo to actually run the agent.

### kubo — the room primitive

A room is a Docker container where agents work. Multi-project bind
mounts, persistent volumes (survive updates), credential passthrough,
idle timeout. A single Rust binary with the Docker image baked in.

```bash
kubo new myroom ~/project           # create a room
kubo add myroom ~/.abot/kubos/...   # mount an agent's home into it
kubo ls                             # list rooms
kubo stop myroom                    # tear down
```

kubo doesn't know about agents. It just manages containers with
bind-mounted directories. The caller decides what gets mounted.

### tao — the human primitive

Treats a human as a Unix process. Blocks a pipeline on a human's
reply, resumes when the reply arrives. Composes via standard pipes.

```bash
echo "Approve this draft?" | tao approve reviewer felix
# blocks until felix replies via email/Telegram
# stdout = felix's reply
# pipeline continues
```

Supports three modes:
- **Blocking** — waits for reply, writes to stdout
- **Detached** — returns tracking ID, resume later with `tao resume`
- **Fire-and-forget** — send and exit, no reply expected

State in `~/.tao/`. IMAP polling daemon for email replies. SQLite for
suspend/resume. See [tao README](https://github.com/Dorky-Robot/tao).

## How they compose

The primitives don't depend on each other at the library level. They
compose via the filesystem and shell — the same way Unix tools always
have.

### Example: auto-draft email replies

```bash
# 1. Mail arrives, gets triaged (humOS only, no agents needed)
humos-fetch && humos-triage

# 2. humOS notices an actionable email, spins up an agent
abot clone alice alice-draft-123
kubo new draft-room-123 ~/context
abot employ alice-draft-123 draft-room-123
kubo add draft-room-123 ~/.abot/kubos/draft-room-123/alice-draft-123/home

# 3. Agent works inside the room (kubo exec)
kubo exec draft-room-123 -- claude "Draft a reply to this email: ..."

# 4. Agent drafts, then asks the human for approval (tao)
cat draft.txt | tao approve reviewer felix

# 5. Human approves, email sends
humos-send gmail reply-123

# 6. Agent integrates experience, room tears down
abot integrate alice-draft-123 draft-room-123
abot discard alice-draft-123 draft-room-123
kubo rm draft-room-123
```

### Example: a simple automation without agents

Not everything needs agents. humOS works fine alone:

```bash
# Fetch mail, triage it, show a report
humos-fetch && humos-triage && humos-mail

# Or via the browser UI
humos-web
```

### Example: agents without humOS

abot + kubo work independently for any task:

```bash
# Create an agent, give it a room, let it work on a codebase
abot create coder
kubo new pr-fix ~/my-project
abot employ coder pr-fix
kubo exec pr-fix -- claude "Fix the failing test in src/auth.rs"
abot integrate coder pr-fix
kubo rm pr-fix
```

## Dependency graph

```
humOS (user-facing personal OS)
  ├── shells out to: abot (agent identity management)
  ├── shells out to: kubo (container rooms)
  ├── shells out to: tao  (human approval gates)
  └── standalone:    humos-* binaries (mail, cal, tasks, etc.)

abot (headless CLI)
  └── no runtime dependencies (just git)

kubo (headless CLI)
  └── requires: Docker

tao (headless CLI)
  └── no runtime dependencies (IMAP/SMTP built in)
```

No circular dependencies. Each tool is installable and usable alone.
humOS is the composition layer that wires them together.

## The company metaphor

```
CEO         = the human (approves via tao)
Workers     = abots (AI agents with git-backed identities)
Offices     = kubos (Docker rooms where work happens)
Mailroom    = humos-fetch + humos-triage (inputs)
Outbox      = humos-send (outputs)
Filing      = ~/.humOS/ (the human's data)
HR          = abot create/clone/integrate (agent lifecycle)
```

The company has a flat structure. No manager agents. No hierarchy.
Agents self-organize around signals from the filesystem. When an email
arrives in `~/.humOS/mail/gmail/INBOX/new/` and gets labeled
`actionable` by `humos-triage`, any agent watching that path can pick
it up. The human doesn't assign work — they approve results.

## What abot is NOT (anymore)

abot was previously a spike that included:
- A Flutter spatial canvas UI
- An Axum HTTP/WebSocket server
- A daemon with session management
- Docker container orchestration (now in kubo)
- WebAuthn authentication

All of that is gone. abot is now a headless CLI that manages git-backed
agent identities. Period. If a spatial UI is needed later, it becomes a
separate frontend that talks to humOS's API — not part of abot.

## Supporting tools

The broader Dorky Robot ecosystem includes tools that integrate with
the core stack but are not required:

```
tala        self-hosted notes (Elixir, git-backed per-note history)
sipag       PR automation agent
katulong    web terminal / remote session sharing
diwa        git history knowledge base
tunnels     Cloudflare tunnel management
hulma       Claude Code project scaffolder
sabihin     notification relay
yelo        S3/Glacier file management
```

These are independent projects. Some are used inside kubos (diwa,
sipag). Some are used alongside humOS (tala for notes). None are
required by the core stack.

## Install

```bash
# The full stack
brew install humos        # personal data + automation
brew install abot         # agent identities
brew install kubo         # container rooms (needs Docker)
brew install tao          # human approval gates

# Or just the parts you need
brew install humos        # works alone for mail/cal/tasks
brew install abot kubo    # works alone for agent workflows
```

## Design constraints

1. **No speculative infrastructure.** Every primitive must earn its
   place by solving a concrete need right now.

2. **Filesystem is the contract.** Tools communicate via files and
   pipes, not RPC or shared libraries. This means any tool can be
   replaced without breaking the others.

3. **Agents are optional.** humOS works without abot. Mail fetching,
   triage, calendar — none of that requires agents or Docker.

4. **Humans are in the loop.** Agents don't act autonomously on
   external systems (send email, deploy code) without human approval
   via tao. The human is always the final gate.

5. **One install per tool.** `brew install X` gives you everything X
   needs. No chasing dependencies across repos.

6. **Git is the memory model.** Agent identity, history, and experience
   are stored as git commits. `git log` is the audit trail. `git merge`
   is how experience integrates. No custom database for agent state.
