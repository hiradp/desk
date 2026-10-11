# desk

Build a terminal workspace out of full-width tabs, from one config file.

A small base layout is always there, one command opens a tab for a repo when
you need it, and you can have as many agent tabs as you like, each started
with its own prompt. Runs on [Rex](https://www.superlogical.com/rex).

## Install

```sh
git clone https://github.com/hiradp/desk.git
just --justfile desk/justfile install
```

For completion of commands, targets and sessions, source the setup from your
shell's startup file. Bash, zsh, fish, elvish and powershell are supported.

```sh
desk completions fish | source
```

## Use

```
desk                          build every startup target; safe to rerun
desk open <target> [prompt…]  open the target, or focus it if already open
desk ls                       list sessions, targets and open tabs
desk stop [session]           kill the sessions desk manages, or just one
desk completions <shell>      print shell completion setup
```

```console
$ desk open app
$ desk open investigate why is checkout slow
$ desk ls
main  🏠 main
  notes  startup  📝 notes
  app    demand   📦 app
investigate  🔎 investigate
  investigate  multi    🔎 investigate 1
```

`desk open` takes you to the tab. Plain `desk` leaves your view where it is.

## Configure

`~/.config/desk/config.toml`, or the file named by `DESK_CONFIG`.

```toml
backend = "rex"
shell   = ["/usr/bin/fish", "-C", "{command}"]
include = ["~/work/desk.toml"]

[main]
label = "🏠 main"

[main.notes]
icon = "📝"
dir  = "~/notes"
mode = "startup"

[main.app]
icon = "📦"
dir  = "~/src/app"

[investigate]
label = "🔎 investigate"

[investigate.investigate]
icon    = "🔎"
dir     = "~/src/app"
command = "agent {prompt}"
mode    = "multi"
```

Each top-level table is a session, and each table inside it is a target: a
recipe for a tab.

| Target field | Meaning                                                     |
|--------------|-------------------------------------------------------------|
| `icon`       | Tab label prefix. Required.                                  |
| `dir`        | Working directory, absolute or starting with `~/`. Required. |
| `command`    | Shell code to run in the tab. May contain `{prompt}`.        |
| `mode`       | `startup`, `demand` (the default) or `multi`.                |

| Mode      | Tabs | Opened by                                   |
|-----------|------|---------------------------------------------|
| `startup` | one  | plain `desk`                                |
| `demand`  | one  | `desk open`, which focuses it if it is open |
| `multi`   | many | every `desk open` adds another, numbered    |

- `shell` is the argv every tab runs. Its `{command}` element becomes the
  target's command, so the tab stays open as a shell when the command exits.
  Without `shell`, tabs open the backend's default shell and targets cannot
  have a `command`.
- The words after the target name are joined into one prompt and replace
  `{prompt}` as a single quoted argument. Quoting follows fish's rules.
- `include` pulls in more files that add sessions and targets. A file that
  does not exist is skipped, so one config works across machines.
- Target names are unique across all sessions, and session labels are unique.
- desk finds its tabs by label: `<icon> <name>`, plus a number for `multi`.

## Not yet

- Backends other than Rex.
- Prompt quoting for shells other than fish.
- Pane splits, tab renaming, JSON output.

## License

[MIT](LICENSE)
