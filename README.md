# CtrlOEM3

This extension fixes the issue where 「Ctrl+`」 doesn't work with some CJK keyboards/IMEs in VSCode.

You can download it directly from the releases page. It's automatically built by Github Actions, so you shouldn't have to worry about any tampering.

## Requirements

-   Platform: x64 Windows 10/11
-   VSCode Version: 1.88.0 or later

## Caveats

-   It occupies the hotkey 「Ctrl+`」 globally whenever VSCode is running.
-   It operates only if the window title matches a configurable regular expression, which is `/^(?: - )?Visual Studio Code/` by default.
