# humOS

A personal OS for humans on autopilot — a collection of small, sovereign binaries that own your data, compose over the filesystem, and delegate to best-in-class Unix tools at the edges.

Each binary is a Unix citizen. State lives under `~/.humOS/` as a file-based database — every small fact is a small file, every entity is its own directory. No god config. No database server. No cloud.

## Install

```sh
cargo install --path .
```

A Homebrew tap (`brew tap dorky-robot/humos && brew install humos`) is on the way.

### Runtime prerequisites

- [`mbsync`](https://isync.sourceforge.io/) (package: `isync`) — IMAP → Maildir sync
- [`himalaya`](https://pimalaya.org/himalaya/) — SMTP send

Both are installable via Homebrew (`brew install isync himalaya`).

Optional (for mail triage with a local LLM):
- [Ollama](https://ollama.com/) running a small model locally

## Mail — quick start

```sh
# Add an account (password read from stdin, stored in the OS keyring)
echo -n "$PASSWORD" | humos mail account add gmail you@gmail.com

# List accounts
humos mail account list

# Pull new mail (generates ~/.humOS/mbsyncrc, execs mbsync)
humos mail fetch

# See what arrived
humos mail                       # default: report unread counts + top 10

# Send a message (RFC-822 on stdin, shelled to himalaya)
printf 'From: you@gmail.com\r\nTo: friend@example.com\r\nSubject: hi\r\n\r\nhello\r\n' \
  | humos mail send gmail

# Classify unread with a local LLM
humos mail triage

# Remove an account (dir + keyring entry)
humos mail account remove gmail
```

Every verb supports `--json` for machine callers and `--help` for a short usage note. No interactive prompts — CLIs are designed for agents and scripts first, humans second.

## Binaries

- `humos` — git-style dispatcher; `humos <noun> …` execs `humos-<noun> …`
- `humos-mail` — mail over `~/.humOS/mail/`, wrapping `mbsync` + `himalaya`
- `humos-web` — minimal localhost browser UI at `127.0.0.1:8765`

## Where state lives

```
~/.humOS/
  mail/
    <account>/
      address         imap.host    imap.port
      smtp.host       smtp.port    smtp.login
      password.cmd    (optional)
      INBOX/{new,cur,tmp}/
      Archive/cur/
  mbsyncrc          (regenerated every `humos mail fetch`)
  himalaya.toml     (regenerated every `humos mail send`)
```

Passwords live in the OS keyring under service `humos`, account `<name>`. A `password.cmd` file per account is a power-user override whose stdout is the password.

## Architecture

humOS is a collection of single-purpose binaries. No monolithic CLI with a hundred subcommands. See:

- [docs/vision.md](docs/vision.md) — principles and north star
- [docs/ecosystem.md](docs/ecosystem.md) — how humOS composes with `tala`, `tao`, `kubo`, `yelo`, and the rest of the Dorky-Robot stack

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
