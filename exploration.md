Just to try and demystify this a little bit, all within one place. This is my best understanding, and I appreciate corrections or nitpicking.

Historically, an input for "shift+enter" didn't exist. The program or TUI can only see whatever the terminal application sends to it, and this input combo used to just be sent as "enter" (much like "shift+a" was sent as "A" and "shift+tab" was sent as "btab"). As with most things added to a standard after the fact, it has been done in multiple ways.

An older implementation used additional escape codes for other keys being modified, called `modifyOtherKeys`.

More recently, kitty standardized extended key, to ensure that more of these modifier sequences get passed to the process within the terminal (among many other fixes).

So with that context, here is each application's support for inputs:

| Input | CSI-u | `modifyOtherKeys` | newline |
| - | - | - | - |
| Claude | ❌ | ❌ | ✅ |
| OpenCode | ✅ | ✅ | ❌ |
| Codex | ✅ | ❌ | ✅ |

CSI-u is the sequence `^[[13;2u`

`modifyOtherKeys` is the sequence `^[[27;2;13~`

newline is an explicit `\r` or `\n` or `\x1b\r` or similar

Within tmux, you can explicitly bind `S-Enter` to any one of these escape sequences.

Sources:

xterm: https://www.xfree86.org/current/ctlseqs.html
modifyOtherKeys: ???
CSI: https://sw.kovidgoyal.net/kitty/keyboard-protocol/

