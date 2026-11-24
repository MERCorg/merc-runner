use std::collections::HashMap;
use std::collections::HashSet;

use serde::Deserialize;
use serde::Serialize;

use crate::error::JobSpecError;
use crate::spec::JobSpec;
use crate::spec::Tool;

/// A step with every reference in its args resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedStep {
    pub tool: Tool,

    /// The arguments after the tool name, including any subcommand.
    pub args: Vec<String>,

    /// The files this step declares as outputs, relative to the working
    /// directory.
    pub outputs: Vec<String>,

    /// The (0-based) indices of the earlier steps whose outputs this step
    /// references, in increasing order. The step can start as soon as all of
    /// them have finished.
    pub depends_on: Vec<usize>,
}

/// A job spec whose step order is a valid execution (and dependency) order,
/// and whose references all resolve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPlan {
    pub steps: Vec<ResolvedStep>,

    /// The output files to keep after the job finishes.
    pub keep: Vec<String>,
}

/// One piece of an argument, split on its `$` references.
#[derive(Debug, PartialEq, Eq)]
enum Piece<'a> {
    Literal(&'a str),
    Declare(&'a str),
    Reference(&'a str),
}

/// Splits `arg` into literal text and references. A `$` that does not start
/// `$$`, `$out.<name>` or `$<name>` is rejected rather than passed through:
/// tool arguments essentially never contain `$`, so it is far more likely a
/// typo than an intentional literal.
fn parse_arg(arg: &str) -> Result<Vec<Piece<'_>>, ()> {
    let mut pieces = Vec::new();
    let mut rest = arg;

    while let Some(position) = rest.find('$') {
        if position > 0 {
            pieces.push(Piece::Literal(&rest[..position]));
        }

        let after = &rest[position + 1..];
        if let Some(remainder) = after.strip_prefix('$') {
            pieces.push(Piece::Literal("$"));
            rest = remainder;
            continue;
        }

        let name = &after[..name_len(after)];
        if name.is_empty() || name == "out" {
            return Err(());
        }
        rest = &after[name.len()..];

        match name.strip_prefix("out.") {
            Some(output) => pieces.push(Piece::Declare(output)),
            None => pieces.push(Piece::Reference(name)),
        }
    }

    if !rest.is_empty() {
        pieces.push(Piece::Literal(rest));
    }

    Ok(pieces)
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c == '.' || c == '-'
}

/// The length of the name at the start of `s`. Trailing dots are not part of
/// the name, so `$model.lts.` ends at `lts`.
fn name_len(s: &str) -> usize {
    if !s.starts_with(is_name_start) {
        return 0;
    }

    let end = s.find(|c| !is_name_char(c)).unwrap_or(s.len());
    s[..end].trim_end_matches('.').len()
}

/// Whether `name` is a full name, usable as an output.
pub(crate) fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && name_len(name) == name.len()
}

/// Whether `name` is usable as an input name: a full name that does not
/// collide with the reserved `$out.` declaration prefix.
pub(crate) fn is_valid_input_name(name: &str) -> bool {
    is_valid_name(name) && name != "out" && !name.starts_with("out.")
}

/// Validates `spec` and resolves every step's args.
///
/// Step order in `spec.steps` is taken as execution order; a step may only
/// reference outputs declared by an earlier step (referencing its own or a
/// later step's output is an unknown name), so the array is always already a
/// valid topological order.
pub fn resolve(spec: &JobSpec) -> Result<ResolvedPlan, JobSpecError> {
    if spec.schema_version != crate::spec::SCHEMA_VERSION {
        return Err(JobSpecError::UnsupportedSchemaVersion {
            found: spec.schema_version,
            supported: crate::spec::SCHEMA_VERSION,
        });
    }

    if spec.steps.is_empty() {
        return Err(JobSpecError::EmptyPipeline);
    }

    // Every name in scope, mapped to what a reference to it expands to.
    let mut names: HashMap<&str, &str> = HashMap::new();
    for (name, location) in &spec.inputs {
        if !is_valid_input_name(name) {
            return Err(JobSpecError::InvalidInputName(name.clone()));
        }
        names.insert(name, location);
    }

    // The index of the step that produces each output.
    let mut producers: HashMap<&str, usize> = HashMap::new();
    let mut outputs = HashSet::new();
    let mut steps = Vec::with_capacity(spec.steps.len());
    for (index, argv) in spec.steps.iter().enumerate() {
        let (step, declared) = resolve_step(index + 1, argv, &names, &producers)?;

        // Only now do this step's outputs come into scope, so later steps
        // can reference them but the step itself cannot.
        for output in declared {
            if names.insert(output, output).is_some() {
                return Err(JobSpecError::DuplicateName {
                    step: index + 1,
                    name: output.to_string(),
                });
            }
            outputs.insert(output);
            producers.insert(output, index);
        }

        steps.push(step);
    }

    let keep = spec
        .keep
        .iter()
        .map(|entry| match parse_arg(entry).as_deref() {
            Ok([Piece::Reference(name)]) if outputs.contains(*name) => Ok(name.to_string()),
            Ok([Piece::Reference(name)]) => Err(JobSpecError::UnknownKeep(name.to_string())),
            _ => Err(JobSpecError::MalformedKeep(entry.clone())),
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ResolvedPlan { steps, keep })
}

/// Resolves the step with 1-based number `step` against the names declared
/// before it, where `producers` maps every output among them to the index of
/// the step that declares it. Also returns the names of the outputs the step
/// declares.
fn resolve_step<'a>(
    step: usize,
    argv: &'a [String],
    names: &HashMap<&str, &str>,
    producers: &HashMap<&str, usize>,
) -> Result<(ResolvedStep, Vec<&'a str>), JobSpecError> {
    let (tool, args) = argv.split_first().ok_or(JobSpecError::EmptyStep { step })?;
    let tool = Tool::from_binary_name(tool).ok_or_else(|| JobSpecError::UnknownTool { step, tool: tool.clone() })?;

    let mut outputs: Vec<&'a str> = Vec::new();
    let mut depends_on = Vec::new();
    let args = args
        .iter()
        .map(|arg| {
            let pieces = parse_arg(arg).map_err(|()| JobSpecError::MalformedReference { step, arg: arg.clone() })?;

            let mut resolved = String::with_capacity(arg.len());
            for piece in pieces {
                match piece {
                    Piece::Literal(text) => resolved.push_str(text),
                    Piece::Declare(output) => {
                        if outputs.contains(&output) {
                            return Err(JobSpecError::DuplicateName {
                                step,
                                name: output.to_string(),
                            });
                        }
                        outputs.push(output);
                        resolved.push_str(output);
                    }
                    Piece::Reference(name) => {
                        let value = names.get(name).ok_or_else(|| JobSpecError::UnknownName { step, name: name.to_string() })?;
                        resolved.push_str(value);
                        if let Some(&producer) = producers.get(name) {
                            depends_on.push(producer);
                        }
                    }
                }
            }

            Ok(resolved)
        })
        .collect::<Result<Vec<_>, _>>()?;

    depends_on.sort_unstable();
    depends_on.dedup();

    let step = ResolvedStep {
        tool,
        args,
        outputs: outputs.iter().map(|o| o.to_string()).collect(),
        depends_on,
    };
    Ok((step, outputs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_references_inside_arguments() {
        assert_eq!(
            parse_arg("--output=$out.model.lts,$spec.").unwrap(),
            vec![
                Piece::Literal("--output="),
                Piece::Declare("model.lts"),
                Piece::Literal(","),
                Piece::Reference("spec"),
                Piece::Literal("."),
            ]
        );
    }

    #[test]
    fn parses_escaped_dollar() {
        assert_eq!(parse_arg("a$$b").unwrap(), vec![Piece::Literal("a"), Piece::Literal("$"), Piece::Literal("b")]);
    }

    #[test]
    fn rejects_bare_dollar_and_bare_out() {
        assert!(parse_arg("$").is_err());
        assert!(parse_arg("$.x").is_err());
        assert!(parse_arg("$out").is_err());
        assert!(parse_arg("$out.").is_err());
    }
}
