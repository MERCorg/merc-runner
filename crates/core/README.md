# Overview

This crate defines the job spec format used by `merc-cloud` and provides
validation for it.

## Spec

A job spec is a JSON file describing a pipeline of tool invocations: which
inputs it needs, which tools run on them in which order, and which results
to keep. It is the format `merc-job` runs locally and that will be submitted
to the cloud runner.

The current version is `schema_version: 2`.

## Example

The ABP pipeline from [`examples/abp.json`](../../examples/abp.json): linearise
an mCRL2 specification, explore its state space and reduce the result modulo
branching bisimulation.

```json
{
    "schema_version": $schema_version,
    "inputs": {
        "spec": "s3://merc-cloud-jobs/job-1/input/abp.mcrl2"
    },
    "steps": [
        ["mcrl22lps", "-v", "$spec", "$out.model.lps"],
        ["merc-lps", "explore-explicit", "$model.lps", "--strategy", "bfs", "--output", "$out.model.lts"],
        ["merc-lts", "reduce", "branching-bisim", "$model.lts", "--output", "$out.model.reduced.lts"]
    ],
    "keep": ["$model.reduced.lts"]
}
```

## Fields

| Field            | Required | Description                                                                   |
| ---------------- | -------- | ----------------------------------------------------------------------------- |
| `schema_version` | yes      | The spec version  .                                                                  |
| `inputs`         | no       | Map from input name to its location: an `s3://` URI or a local path.          |
| `steps`          | yes      | Non-empty list of steps, run in order. Each step is the tool's argv.          |
| `keep`           | no       | Outputs to keep after the job finishes, as `$<name>` references.              |

Unknown fields are rejected.

### Steps

A step is a list of strings: the tool name followed by its arguments, exactly
as they would be typed on the command line. Subcommands are ordinary
arguments, e.g. `["merc-lts", "reduce", ...]`.

The tool names are the executable names of the tools available on the runner.

Any other tool is rejected, since a spec drives process execution on the
runner.

## References

Arguments may contain references, which are replaced before the step runs.
A reference can appear anywhere inside an argument, so `--output=$out.a.lts`
works as expected.

| Syntax          | Meaning                                                                    |
| --------------- | -------------------------------------------------------------------------- |
| `$out.<name>`   | Declares an output of this step. Replaced by the file name `<name>`.       |
| `$<name>`       | Refers to an input, or to an output declared by an earlier step.           |
| `$$`            | A literal `$`.                                                             |

Any other use of `$` is an error, since tool arguments essentially never
contain one and it is more likely a typo.

### Names

- A name consists of letters, digits, `_`, `.` and `-`, and starts with a
  letter, digit or `_`. A name ends at the first other character; trailing
  dots are not part of it, so in `$model.lts.` the name is `model.lts`.
- An output's name is also its file name in the step's working directory, so
  include the extension the tool expects (`$out.model.lts`).
- Inputs and outputs share one namespace: every name may be declared only
  once, either as an input or by a single step.
- Input names may not be `out` or start with `out.`, as those would be read as
  output declarations.

### Order

A step can only refer to outputs of steps listed before it; referring to its
own outputs or those of a later step is reported as an unknown name. The list
is therefore always a valid execution order.

A step depends on the steps whose outputs it references, and may start as soon
as those have finished. Steps that do not depend on each other can run in
parallel.

### Threads

Steps are run within a thread budget: the threads used by all running steps
together never exceed it. A step needs one thread, unless its arguments
contain `--threads <n>` (or `--threads=<n>`), in which case it needs `n`
(capped to the budget, so such a step then runs on its own).

Tools that can be told how many threads to use will in the future get a share
of the free threads, passed as `--threads <share>`: two such steps starting
together with a budget of 8 get 4 threads each. This is not enabled for any
tool yet.

### Keep

Each `keep` entry must be exactly one `$<name>` reference to a step's output.
Inputs cannot be kept. Outputs that are not kept are intermediates and may be
discarded once the job finishes.

## Running a spec locally

```sh
merc-job examples/abp.json            # run every step
merc-job --dry-run examples/abp.json  # validate and print the commands
merc-job --workdir out/ examples/abp.json
merc-job --threads 4 examples/abp.json  # use at most 4 threads at once
```

- Independent steps run in parallel, using at most `--threads` threads at
  once (default: the number of available CPUs). If a step fails, the steps
  still running are killed and no further steps start.
- A step's stdout and stderr are merged in the order the tool wrote them.
  Every line is echoed to stderr prefixed with the step number and tool, e.g.
  `[3 merc-lps] ...`, and written unchanged to `<step>-<tool>.log` in
  `--log-dir` (default: `logs` inside the working directory). A failed step's
  error message names its log.

- Steps run in `--workdir` (default: the current directory), so outputs are
  written there.
- Local input paths are relative to the directory of the spec file.
- `s3://` inputs are not yet supported when running locally; a run stops with
  an error. `--dry-run` still accepts them, so specs meant for the cloud can be
  validated locally.
- After a successful run the paths of the kept outputs are printed.
- `--merc-path` and `--mcrl2-path` point at the directories holding the merc
  and mCRL2 binaries; by default they are looked up on `PATH`.

A dry run of the example prints:

```text
[1] mcrl22lps -v s3://merc-cloud-jobs/job-1/input/abp.mcrl2 model.lps
[2] merc-lps explore-explicit model.lps --strategy bfs --output model.lts  (after 1)
[3] merc-lts reduce branching-bisim model.lts --output model.reduced.lts  (after 2)
keep model.reduced.lts
```


## Compositions

`merc-job --composition composition.json` reads a compositional generation
description instead of a job spec (see [`composition`](src/composition.rs)),
converts it into a job spec and runs that as normal. The conversion produces:

1. per machine part: `mcrl22lps <mcrl2_file> <lps_file>` followed by
   `merc-lps explore-explicit <lps_file> --output <lts_file>`;
2. per composition step: `merc-lts combine <left> <right> --output <output_lts_file>`
   with `--comm`, `--allow` and `--hide` sets (omitted when empty).

An operand named `x.<default_reduction>.lts`, for an `x.lts` produced by a
part or an earlier step, is first produced by `merc-lts reduce`. The mCRL2
equivalence names `bisim`, `dpbranching-bisim` and `dpweak-bisim` are mapped
to their merc names; other names are passed through unchanged. The output of
the last composition step is kept.

All file names must be plain names (letters, digits, `_`, `.` and `-`), since
they become `$` references in the generated spec.
