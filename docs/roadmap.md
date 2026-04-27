# Roadmap — humOS, tao, abot

The sequence we're committing to so the three projects converge into one
coherent system. Each phase has a single deliverable, unblocks the next,
and ends with something brew-installable and useful in isolation.

The DCI (Data, Context, Interaction) layer — actors playing roles inside
contexts under policy — lives in **humOS**. tao stays the
pipe-to-a-human primitive. abot stays the agent-identity primitive.
Neither tao nor abot needs to know about roles; humOS binds and
orchestrates.

## Architecture in one breath

```
                ┌──────────────────────────────────────────┐
                │   humOS — DCI orchestration              │
                │   contexts, roles, policies, bindings    │
                └───────┬─────────────────────┬────────────┘
                        │                     │
            ┌───────────▼─────────┐ ┌─────────▼──────────┐
            │ tao                 │ │ humos-mail         │
            │ pipe-to-a-human     │ │ humos-* (data)     │
            │ suspend/resume      │ │ Maildir, etc.      │
            └───────────┬─────────┘ └────────────────────┘
                        │
                ┌───────▼─────────┐
                │ abot            │
                │ agent identity  │
                │ git-backed      │
                └─────────────────┘
```

humOS calls tao to invoke an actor. tao reaches the actor — human (via
humos-mail) or agent (via abot). Each layer is replaceable; nothing
upstream knows the medium.

## Decisions already locked in

- abot rewrite happens **on the existing repo**. History stays; spike code is replaced in-place. Diwa insights and `git log` continue to work.
- abot phase-1 stop line is **the twelve verbs from `docs/rewrite.md` plus a 13th, `abot run`** — LLM dispatch lives inside abot so an agent is a Unix program, not a record. No kubo integration, no tao integration, no humOS integration.
- abot v1 LLM provider: **Ollama only**. Local-first, no API auth, matches the model-selection pattern already in `humos mail triage`.
- Brew distribution via `dorky-robot/homebrew-tap` for every shipped binary.
- Naming: `abot` is the kind and project; `alice`, `bob`, etc. are instance names.
- DCI vocabulary (actor / role / context / interaction) lives only in humOS. abot says "agent." tao says "actor + action."
- **Dev environment for any work that exercises `abot run`:** `ssh mac2024`. The dev machine doesn't have the RAM headroom for local models; mac2024 does. Code editing can happen anywhere; integration tests that hit Ollama run on mac2024.

---

## Phase 1 — Simplify abot

**Goal:** abot is a headless CLI that manages agent identities as git
repos *and* runs them as Unix programs. Drop the Flutter client, Axum
server, daemon, Docker, WebAuthn, sessions, ring buffers — everything
that isn't thirteen verbs.

**Deliverable:** `brew install dorky-robot/tap/abot` gives you the
headless CLI. `echo "hi alice" | abot run alice` invokes the agent.
~1800–3000 lines of Rust (slightly larger than the original plan to
account for the LLM dispatch path).

**Out of scope:** kubo composition, tao composition, humOS composition.
Those are later phases.

### Tasks

- [ ] Open a `headless` branch on the existing abot repo
- [ ] Delete `flutter_client/`, `e2e/`, `playwright.config.ts`, `package*.json`, `Dockerfile.session`, `mkdocs.yml`
- [ ] Delete `src/server/`, `src/stream/`, `src/auth/`, `src/daemon/` (everything that isn't the new CLI)
- [ ] Strip `Cargo.toml` to: `clap`, `serde`, `serde_json`, `chrono`, `anyhow`, `ureq` (for Ollama HTTP). No `axum`, `tokio`, `bollard`, `webauthn-rs`, `rust-embed`, `rusqlite`, `tracing`
- [ ] Implement `paths.rs` — resolve `~/.abot/` and the agent/kubo subpaths
- [ ] Implement `git.rs` — shell-out helpers (`git init`, `worktree add/remove/list`, `branch`, `merge`, `clone`, `log`, `diff`)
- [ ] Implement `manifest.rs` and `config.rs` — read/write the JSON files
- [ ] Implement `agent.rs` — `create`, `list`, `show`, `rm`, `config`
- [ ] Implement `clone.rs` — full repo copy + manifest rename
- [ ] Implement `employ.rs` — `employ`, `dismiss`
- [ ] Implement `integrate.rs` — `integrate`, `discard`
- [ ] Implement `log` and `diff` verbs
- [ ] Add a `model` field to `config.json` (default `"llama3"` or whichever Ollama model we settle on)
- [ ] Implement `run.rs` — read agent config, read stdin, POST to `http://localhost:11434/api/chat` with `instructions` as system prompt, stream stdout. Strict stdin → LLM → stdout; no tool use, no multi-turn, no vector DB
- [ ] `abot run alice` runs in alice's canonical `home/`; `abot run alice --in <room>` runs in the worktree from `employ`
- [ ] `main.rs` — clap dispatch for all thirteen verbs
- [ ] Tests: one integration test per verb against a temp `~/.abot/` (TDD per the humOS feedback memory — write the failing test first). Git verbs test on the dev machine; `run` integration test runs against a live Ollama on `ssh mac2024`
- [ ] Update README to match the headless model; archive the old README content under `docs/spike-readme.md`
- [ ] Delete `BRAINSTORM.md` and `SCRATCHPAD.md` (or move under `docs/spike/`)
- [ ] Bump `Cargo.toml` to `1.0.0` (clean break from the spike's `0.x`)
- [ ] Build with the path-remap RUSTFLAGS (per `reference_brew_tap.md`)
- [ ] Cut release tarball and SHA
- [ ] Update `Formula/abot.rb` in the tap to point at the new tarball
- [ ] Verify `brew install dorky-robot/tap/abot` works on a clean machine

**Phase 1 done when:** on a fresh install with Ollama running, all of these work end-to-end with nothing else in place:
- `abot create alice && abot employ alice test-room && abot integrate alice test-room`
- `echo "say hello in one word" | abot run alice` (returns a one-word reply)
- `echo "summarize this" | abot run alice --in test-room` (runs in the worktree)

---

## Phase 2 — Collapse tao's email channel onto humos-mail

**Goal:** one IMAP poller on the machine, one source of truth for mail.
tao stops re-implementing what humos-mail already does.

**Deliverable:** `tao-channels::imap` is gone. `tao` depends on the
Maildir layout (or shells to `humos mail`).

**Out of scope:** changing tao's actor / role / action vocabulary.
Channels remain a tao implementation detail; we just swap the email
implementation.

### Tasks

- [ ] Decide: tao reads Maildir directly from `~/.humOS/mail/<account>/` *or* shells to `humos mail ls --json` / `cat`. Pick one and document it
- [ ] Write reply-matching tests against a fixture Maildir (TDD)
- [ ] Replace `tao-channels::email` send path with `humos mail send` (RFC-822 on stdin)
- [ ] Replace `tao-channels::imap` poll loop with Maildir watch (or `humos mail ls --unread --json` polled at the same cadence)
- [ ] Delete the `imap` crate dep from `tao-channels/Cargo.toml`
- [ ] Delete `tao-channels/src/{imap.rs,email.rs,email_util.rs}` (keep `channel.rs`, `telegram.rs`, `websocket.rs`)
- [ ] Update tao's daemon — no more long-lived IMAP connection; reply-watch is filesystem-driven
- [ ] Documentation: update tao README's "How it works" — replies arrive via humOS, not tao's own poller
- [ ] Cut a tao release with the new architecture
- [ ] Update `Formula/tao.rb` if/when tao ships through the tap

**Phase 2 done when:** running tao with `tao-channels::imap` removed still completes a real `tao approve` workflow against Felix's Gmail end-to-end.

---

## Phase 3 — Teach tao how to invoke abot actors

**Goal:** tao can route to an abot actor the same way it routes to a
human.

**Deliverable:** `tao approve developer alice` works whether `alice` is
a human (email) or an abot (agent record).

**Out of scope:** roles, contexts, policies. tao still treats actors as
flat; the role layer is humOS's job in phase 4.

### Tasks

- [ ] Add `kind: "abot" | "human"` to tao's actor record
- [ ] Resolve abot actors via `abot show <name>` (read manifest + config; the `instructions` field becomes the system prompt)
- [ ] Implement abot-actor invocation: tao shells out to `abot employ alice <session-id>` (if isolation is wanted) and then `abot run alice --in <session-id>`, capturing stdout. On success: `abot integrate`. On failure: `abot discard`
- [ ] Tests: a fake-LLM `Runner` (or a stub `abot run` that echoes canned text) exercised through tao's suspend/resume path. No live Ollama in tao's test suite
- [ ] Document the contract: an abot actor's stdin is the same shape a human would receive; its stdout is treated as the reply

**Phase 3 done when:** the same tao pipeline works with a human actor and an abot actor swapped in, with no other changes.

---

## Phase 4 — humOS context / role / policy layer

**Goal:** humOS gains a first-class DCI primitive — contexts, roles
inside contexts, role policies, bindings of actors to roles.

**Deliverable:** a new humOS binary (working name: `humos-room`) with a
file-based DB matching humOS conventions.

**Out of scope:** any specific scenario. This phase is the primitive,
not the use case. The mailroom is phase 5.

### Tasks

- [ ] Decide naming: `humos room` vs `humos ctx` vs `humos context`. My read: `room` (short, Unix-flavored, matches the user's "mailroom" vocabulary)
- [ ] Design the on-disk shape: `~/.humOS/contexts/<ctx>/{roles/<role>/{policy,state,before,after}, bindings, state, log}` — one file per fact, no god files (per `feedback_file_based_db.md`)
- [ ] Write the file-shape spec into `docs/dci.md` so it's the source of truth
- [ ] Implement `humos room create <name>` — initializes the directory
- [ ] Implement `humos room ls` — list contexts with role/binding counts
- [ ] Implement `humos room show <ctx>` — print roles, current bindings, state
- [ ] Implement `humos room role add <ctx> <role>` — adds `roles/<role>/`
- [ ] Implement `humos room bind <ctx> <role> <actor>` — writes a binding (actor name + kind)
- [ ] Implement `humos room unbind <ctx> <role>` — removes the binding
- [ ] Implement `humos room step <ctx> <role>` — read stdin, dispatch through tao to the bound actor, write stdout. The DCI interaction lives here
- [ ] Implement `humos room state <ctx>` — print/update context-shared scratch (the score in the baseball game)
- [ ] Hook plumbing: `roles/<role>/before` and `after` execute around `step`, same shape as tao's existing action hooks
- [ ] Tests for every verb, file-based fixtures
- [ ] Update `Formula/humos.rb` to install the new binary
- [ ] Document in `docs/dci.md` how the layer composes with tao and abot — concrete example with one role, one actor

**Phase 4 done when:** `echo "hi" | humos room step demo greeter` works end-to-end with a bound abot or human actor.

---

## Phase 5 — Mailroom MVP

**Goal:** the first real DCI scenario, daily-driven on Felix's actual
inbox. Forces the rough edges across all four prior phases.

**Deliverable:** Felix triages his unread mail every morning through
the mailroom. Alice (abot) does the first pass; Felix (human) handles
escalations.

**Out of scope:** more contexts (calendar, tasks, etc.). Those come
after the mailroom proves the model.

### Tasks

- [ ] `humos room create mailroom`
- [ ] `humos room role add mailroom triager`
- [ ] `humos room role add mailroom escalator`
- [ ] `abot create alice` with `instructions` tuned for triage
- [ ] `humos room bind mailroom triager alice`
- [ ] `humos room bind mailroom escalator felix` (human, reachable via humos-mail)
- [ ] Write `roles/triager/policy` — output JSON `{label, confidence, reason}`, schema-validated by the `after` hook
- [ ] Write `roles/escalator/policy` — only invoked when triager reports `urgent` with confidence ≥ 0.8
- [ ] End-to-end pipeline: `humos mail ls --unread --json | humos room step mailroom triager | humos room step mailroom escalator --when urgent | humos mail send`
- [ ] Daily cron / launchd plist that runs the pipeline at 7am
- [ ] Two-week soak: drive it daily, capture rough edges, file follow-ups
- [ ] Document the scenario as a working example in `docs/dci.md`

**Phase 5 done when:** the morning pipeline has run unattended for two
weeks and Felix prefers it to manually opening Gmail.

---

## Tracking

- Mark phases complete by checking every box and updating the **Phase N done when** line with a date.
- Anything that emerges mid-phase but doesn't belong in the current
  phase goes into a "Deferred" section at the bottom of the relevant
  phase, not into the next phase. Phases stay scoped.

## Deferred (cross-cutting, parked)

- Intel x86_64 release builds for humOS (currently aarch64-only)
- `humos --help | head` SIGPIPE handling
- 506 Gmail duplicates from humos-mail v0.1.0 first-fetch — manual cleanup
- abot–tao daemon-pattern convergence (premature; revisit after a third example)
- alon (codebase visualizer) integration with the humOS Rust workspace — orthogonal track
