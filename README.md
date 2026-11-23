# webgrab

Scan a web page (or a saved .html file) for links and download the files: images, videos, anything.
It can also crawl through linked pages, remember what it did in a database, and draw a graph of it.

**Install:** `cargo build --release` -> `target/release/webgrab` (& `./install.sh`)

```sh
webgrab <URL|FILE> [flags]       scan one page
webgrab -r urls.txt [flags]      download every URL listed in a file
webgrab -w [flags]               download URLs as you copy them
```

## Examples

Download all images from a page:
```sh
webgrab https://example.com/gallery --image
```

Images and videos, 4 downloads at once, into a folder:
```sh
webgrab https://example.com/gallery --media -t 4 -d out
```

Just print the links, don't download (one per line, pipe-friendly):
```sh
webgrab https://example.com/gallery --image --list
webgrab https://example.com/gallery --image --list > links.txt
```

Only some links (the link must contain...):
```sh
webgrab https://example.com --any .jpg,.png --must /full/ --ignore thumb
```

Follow links to other pages too (depth 2 = the page, its links, and their links):
```sh
webgrab https://example.com/posts --image --crawl-depth 2 --unique
```

Crawl only some pages and go slowly:
```sh
webgrab https://example.com -D 2 --image --crawl-must /post/ --crawl-sleep 1000
```

Don't keep the same picture twice (even under different URLs):
```sh
webgrab https://example.com --image --nodupes
```

Remember everything in a database, so a stopped run can be started again and picks up where it left:
```sh
webgrab https://example.com -D 3 --media --nodupes --unique --db crawl.db
```

Look at what a crawl found:
```sh
webgrab --tree crawl.db                   # tree of pages and files in the terminal
webgrab --db crawl.db --graph crawl.html  # interactive graph, open in a browser
```

A saved .html file (add `--base` so relative links work):
```sh
webgrab page.html --base https://example.com --image
```

One URL per line from a file:
```sh
webgrab -r urls.txt --threads 4
```

Look like a browser, one live counter line:
```sh
webgrab https://example.com --image --agent-firefox --compact
```

## Good to know
- Flags work with one or two dashes and hyphens or underscores: `--crawl-depth` = `-crawl-depth` = `--crawl_depth`.
- Short flags (`-t 4`, `-d out`, `-D 2`) take their value after a space and can't be combined.
- Files that already exist are renamed (`name [1].ext`); `--skip` skips same names, `--replace` overwrites.
- HTTP 429 (too many requests) is handled automatically, for crawled pages and for file downloads: it retries and learns a pause so it doesn't happen again. `--sleep MS` / `--crawl-sleep MS` set a minimum pause, `--no-retry-429` turns it off.
- Colors are on in a terminal; `--colorless` turns them off.

## More
- [DOCS.md](DOCS.md): every flag, how crawling and rate limits work, resuming, the database tables, graph, tree.
- `webgrab --help`: all flags with explanations.
