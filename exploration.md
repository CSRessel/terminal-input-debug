Just to try and demystify this a little bit, all within one place. This is my best understanding, and I appreciate corrections or nitpicking.

Historically, an input for "shift+enter" didn't exist. The program can only see whatever the terminal application sends to it, and this "shift+enter" input combo is historically just sent as the single key for "enter" (much like "shift+a" was sent as "A" and "shift+tab" was sent as "btab").^1
As with most things added to a standard after the fact, it has been done in multiple ways.

An older implementation used additional escape codes for other keys being modified, called `modifyOtherKeys`.^2

More recently, kitty created extended keys.^3 This newer spec ensures that more of these modifier sequences get passed to the process within the terminal (among many other fixes). It adds a new set of control sequence identifiers to do this, called "CSI u".

So with that context, here is each application's support for each input at the time of writing:

| Input     | CSI u | `modifyOtherKeys` | newline |
| -         | -     | -                 | -       |
| OpenCode  | ✅    | ✅                | ❌      |
| Claude    | ❌    | ❌                | ✅      |
| Codex     | ✅    | ❌                | ✅      |

- CSI u uses the sequence `^[[13;2u`
- `modifyOtherKeys` uses the sequence `^[[27;2;13~`
- newline is sending the explicit character of `\r` or `\n` or `\x1b\r` or similar

So hopefully this explains why:
1. Claude changes the config (they aren't properly handling the other escape sequence options)
2. Claude's terminal config change breaks OpenCode (which isn't handling the newline character)
3. No one has a config which works for both

And then for those who have issues within tmux, you can explicitly bind `S-Enter` to any one of these input sequences.

```
# Pick your poison:

# Works with Claude and Codex
# bind S-Enter send-keys "\n"

# Works with OpenCode and Codex
# bind S-Enter send-keys "^[[27;2;13~"

# Works with OpenCode and Codex
set -g extended-keys on
bind S-Enter send-keys "^[[13;2u"
```

Sources:

- 1 xterm: https://www.xfree86.org/current/ctlseqs.html (or [the manual pdf](https://invisible-island.net/xterm/ctlseqs/ctlseqs.pdf))
- 2 `modifyOtherKeys`: https://invisible-island.net/xterm/modified-keys.html
- 3 extended keys: https://sw.kovidgoyal.net/kitty/keyboard-protocol/

