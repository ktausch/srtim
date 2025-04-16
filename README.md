# srtim

The `srtim` library represents and provides utilities to create speedrun runs (with death tracking for video games). It has no dependencies except for the Rust standard library `std`.

The `srtim` binary allows users to add/delete segments, add/delete groups of segments, and run single segments or groups of segments.

## Commands

The `srtim` command has the following subcommands:

1. `help`
2. `add`
3. `delete`
4. `run`

Use `srtim help` to print details about all of the subcommands.

```bash
srtim help # prints a help message about subcommands and their arguments
srtim add --id-name a --display-name "segment A :)" # adds a segment
srtim add --id-name b --display-name "segment B :)" # adds another segment
srtim add --id-name ab --display-name "GROUP" --parts a b # adds segment group
srtim run --id-name a # interactively runs the first single segment
srtim run --id-name b # interactively runs the second single segment
srtim run --id-name ab # interactively times the two segment run
# now, ab has one run, a has two runs, and b has two runs
srtim delete --id-name a --recursive # deletes "a" and "ab"
srtim delete --id-name b # deletes "b", leaving config empty
```

## On-disk storage

### The configuration file

The on-disk storage of how segments and segment groups relate to each other is the srtim config file, stored by default at `~/srtim/config.tsv` (the exact directory can be specified with a `--root` option to the binary). It is a tab-separated values file where each row (after the header) stores one segment or group of segments. Each part has an ID name (a short string with which to refer to the part which cannot have tabs or colons) and a display name (which is what is shown during interactive runs). There is another column in the file that is used only by segment groups and not by single segments: the parts list. The parts list is a colon separated list of ID names that the segment group consists of. These part ID names can refer to single segments or groups of segments.

### Segment times files

The other on-disk storage used by `srtim` is one file per part that has ever been run. These are stored at `${id_name}.csv` in the same directory where the config file is stored.
