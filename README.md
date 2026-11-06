# avkean 词典

## What it is
- A fast, quiet Chinese–English dictionary for everyday lookups.
- Common searches are answered in the browser, without a server call.
- The server doesn't log searches or IP addresses.

## What you can do
- Search by characters, pinyin (with or without tones) or English.
- Draw a character with the handwriting pad.
- Wildcards match prefixes: `?` covers one character, `*` zero to three, and words can continue after the pattern (`中?`, `zhong1*ren2`).
- `#` at the start searches headwords, meanings and examples: `#rain`.

## How it works
- Meanings come from CC-CEDICT, English Wiktionary and the dictionary's own entries, merged into one record per word and reading.
- English results start with a curated list of the usual words, then the closest matches.
- Chinese and pinyin results go by how common the word is.

## Running it yourself

Requires Python 3.11+, Rust 1.99.1 (verified), and Unix (Linux or macOS).

```sh
python3 scripts/build_dictionary.py --with-core
cargo run --release
```

- Then open http://127.0.0.1:3000.
- Settings: `CIDIAN_HOST` (default `127.0.0.1`, must be an IPv4 or IPv6 address), `CIDIAN_PORT` (default `3000`), `CIDIAN_DATA` (default `data`) and `CIDIAN_FEEDBACK_URL` (default empty, an optional feedback link).
- Web files are built into the binary; rebuild after editing `web/`.

## Deploying
Requires Docker Engine 25 or newer with BuildKit and Compose.

- Build the data, then the image: `docker build -t cidian .`
- `compose.yml` runs it read-only with a 1 GB memory limit and 2 CPUs.
- Run `docker network create proxy_net`, then `docker compose up -d`.
- Put a reverse proxy in front for HTTPS and join it to `proxy_net`; it reaches the app as `cidian:3000` on that network.
- The proxy or CDN must overwrite `X-Real-IP` with the visitor's IP, and the app must not be reachable directly. The limit is 600 API requests a minute; without the header, all visitors share one allowance.

## Sources and licences
- The code is MIT ([LICENSE](LICENSE)). The data keeps the licences listed in NOTICE.
- The build downloads fixed copies of CC-CEDICT, Wiktionary and Tatoeba from this repo's [sources-2026-11-02 release](https://git.avkean.com/avkean/cidian/releases/tag/sources-2026-11-02) and checks each hash. The four files and [data/licenses/NOTICE](data/licenses/NOTICE) are release attachments.
- Dictionary text is CC BY-SA 4.0. Tatoeba examples are CC BY 2.0 FR and show their authors.
- Stroke data is from Make Me a Hanzi.
- [data/licenses/NOTICE](data/licenses/NOTICE) lists the sources; the licence texts that must travel with the data are next to it.
