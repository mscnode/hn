# hn

A modern, fast and lightweight Hacker News command‑line client written in Rust.
It scrapes the live Hacker News website and displays stories, details and user information directly in your terminal.

## Features

- Browse Top, New, Best, Ask HN, Show HN and Job stories

- Open stories or discussions in your default browser

- View basic story details and first comments

- Display basic user profile information

- Simple, text‑based cache with TTL (no JSON, no Serde)

- Async HTTP client with connection pooling for good performance

---

## Installation

## Prerequisites

- Rust toolchain (stable) with `cargo` installed
  You can install it from: [**https://rustup.rs/**](https://rustup.rs/)

## Build from source

```bash
git clone https://github.com/mscnode/hn.git
cd hn
cargo build --release 
```

The compiled binary will be located at:

```plaintext
target/release/hn
```

---

## Usage

Basic syntax:

```bash
hn [COMMAND] [OPTIONS] 
```

If no command is provided, it defaults to `top` (Top stories, page 1).

## Top stories

List top stories (default):

```bash
hn top
```

Options:

- `-p, --page <NUMBER>`: Page number to fetch (default: `1`)

## New stories

```bash
hn new
```

## Best stories

```bash
hn best
```

## Ask HN

```bash
hn ask
```

## Show HN

```bash
hn show
```

## Job stories

```bash
hn job
```

---

## Details and users

## Story details

Show basic details and a short preview of the first comments for a specific item ID:

```bash
hn details <id> 
```

Arguments:

- `<id>`: Hacker News item ID (e.g. `40000000`)

## User info

Show basic information for a Hacker News user:

```bash
hn user <username>
```

Arguments:

- `<username>`: Hacker News username

---

## Opening in the browser

The CLI keeps a small cache of the last fetched stories and lets you open them quickly.

After running a listing command (`top`, `new`, `best`, `ask`, `show`, `job`), you can open a story by its rank:

```bash
hn open <rank>
```

Behavior:

- If the story has an external URL, that URL is opened in your default browser

- If it does not, the Hacker News discussion page is opened instead

Arguments:

- `<rank>`: The rank number shown in the list (e.g. `1`, `10`, `25`)

---

## Parallel multi‑page fetch

Fetch multiple pages in parallel for faster scraping (if you enabled the `multi` command in the code):

```bash
hn multi
hn multi --category top --num-pages 3
hn multi -c ask -n 5
hn m -c new -n 2 
```

Options:

- `-c, --category <top|new|best|ask|show|job>`: Story category (default: `top`)

- `-n, --num-pages <NUMBER>`: Number of pages to fetch in parallel (default: `3`)

The results are flattened, cached and displayed as a single list.

---

## License

MIT License
