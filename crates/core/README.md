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

Steps run in the order they are listed. A step can only refer to outputs of
steps before it; referring to its own outputs or those of a later step is
reported as an unknown name.

### Keep

Each `keep` entry must be exactly one `$<name>` reference to a step's output.
Inputs cannot be kept. Outputs that are not kept are intermediates and may be
discarded once the job finishes.

## Running a spec locally

```sh
merc-job examples/abp.json            # run every step
merc-job --dry-run examples/abp.json  # validate and print the commands
merc-job --workdir out/ examples/abp.json
```

- Steps run in `--workdir` (default: the current directory), so outputs are
  written there.
- Local input paths are relative to the directory of the spec file.
- `s3://` inputs are not yet supported when running locally; a run stops with
  an error. `--dry-run` still accepts them, so specs meant for the cloud can be
  validated locally.
- After a successful run the paths of the kept outputs are printed.

A dry run of the example prints:

```text
[1] mcrl22lps -v s3://merc-cloud-jobs/job-1/input/abp.mcrl2 model.lps
[2] merc-lps explore-explicit model.lps --strategy bfs --output model.lts
[3] merc-lts reduce branching-bisim model.lts --output model.reduced.lts
keep model.reduced.lts
```

