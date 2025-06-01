# Integrations

This directory contains integrations between IronGRP and third party software.


## Shell completion

IronGRP has built in support for generating shell completions, using the `--generate-shell-completions` flag. To generate completions for zsh, for example, run this:

```
$ ./irongrp --generate-shell-completions zsh | sudo tee /usr/local/share/zsh/site-functions/_irongrp
$ compinit
```


## ImHex

[ImHex](https://github.com/WerWolv/ImHex) is a very capable open source hex editor. Included in the `imhex` directory are hexpat files for parsing and highlighting normal GRP files, Uncompressed GRP files and WarCraft I style GRP files.


## Yazi

[Yazi](https://yazi-rs.github.io/) is an open source terminal file manager. Included here is a plugin that integrates IronGRP to the preview panel of Yazi, so that the content of .tbl files can be seen when they are selected. Move the `irongrp.yazi` directory to `~/.config/yazi/plugins`. Then add this to `~.config/yazi/yazi.toml`:

```
[plugin]
prepend_previewers = [
    { name = "*.grp", run = "irongrp"},
    { name = "*.gfx", run = "irongrp"},
    { name = "*.gfu", run = "irongrp"},
]
```

The .gfx and .gfu extensions were used by mods from the WarCraft I and II modding days; if you don't anticipate that you will interact with old WarCraft I or II mods, you can choose to only keep the prepend previewer for .grp files, as that is the name used by Blizzard.
