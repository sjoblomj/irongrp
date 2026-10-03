# irongrp.yazi
This is a Yazi plugin for previewing GRP files from Warcraft I, Warcraft II
and StarCraft using [IronGRP](https://github.com/sjoblomj/irongrp).

## Selecting a palette

By default the GRP is rendered using the most recently used palette from the
history. If the history is empty, or the most recent entry no longer points to
a readable file, the preview falls back to greyscale (no palette).

To change the palette, bind a key to the plugin's entry in your `keymap.toml`
(by default located in `~/.config/yazi/keymap.toml`), for example:

```toml
[[manager.prepend_keymap]]
on   = [ "i" ]
run  = "plugin irongrp"
desc = "Set palette for GRP preview"
```

While previewing a GRP file, press the bound key (`i` in the example above) to
open a menu with:

- The most recently used palettes (up to 9), each bound to a digit key.
- `i` — Type a new palette path in a text input. Submitting an empty value
  switches to greyscale for the hovered file.
- `g` — Switch the hovered file to greyscale (no palette).

`~`, `$VAR` and `${VAR}` in the typed path are expanded. The choice is
remembered per file for the current Yazi session, and the palette history is
persisted across sessions to `$XDG_STATE_HOME/yazi/irongrp-palettes`
(or `~/.local/state/yazi/irongrp-palettes`).
