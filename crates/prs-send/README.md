# `prs-send`

`prs-send` is the trusted host CLI for PRSync. It uses the Worker sender
routes and does not call reader manifest or bundle routes.

Set the Worker URL before running a command:

```sh
export PRSYNC_URL=https://sync.example.com
```

Create a named sender credential with human approval:

```sh
prs-send credentials create --name laptop
```

The command prints the bearer token once to standard output. It prints the
approval URL and progress messages to standard error. Store the token outside
the CLI if later commands need it:

```sh
export PRSYNC_SENDER_TOKEN='the-token-printed-by-create'
prs-send push docs/index.md docs/images/diagram.png
prs-send clear
prs-send credentials list
prs-send credentials revoke laptop
```

`push` includes only the paths supplied on the command line. The first path is
the Markdown entry point. The CLI computes the common parent directory and
stores the remaining paths in the generated bundle manifest. It does not read
the hosted inbox after a push.

Run the focused tests from the repository root:

```sh
cargo test -p prs-send
cargo clippy -p prs-send --all-targets -- -D warnings
```
