# CtrlOEM3

> Check your IME's preferences [first][blog], as the hotkey is likely configurable.

[blog]: https://lanlan.moe/blog/fix-vscode-ctrl-backquote/

This extension fixes the issue where 「Ctrl+`」 doesn't work with some CJK keyboards/IMEs in VSCode.

You can download it directly from the releases page. It's automatically built by GitHub Actions, so you don't have to worry about any tampering.

## Requirements

-   Platform: x64 Windows 10/11
-   VS Code 1.88.0 or later

## Caveats

-   It registers the global hotkey 「Ctrl+`」 while VS Code is running.
-   It only acts when the foreground window title matches a configurable regular expression. The default is `/^(?: - )?Visual Studio Code/`.
