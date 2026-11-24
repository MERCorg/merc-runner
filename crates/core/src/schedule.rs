//! Schedules the steps of a [`ResolvedPlan`] so that independent steps run in
//! parallel, while the threads used by all running steps together never
//! exceed a fixed budget.
//!
//! Every step has a [`ThreadDemand`]: most tools are single threaded and need
//! exactly one thread, a step whose args already contain `--threads <n>` needs
//! `n`, and a step whose tool can be told how many threads to use is
//! [`ThreadDemand::Flexible`]. Flexible steps share the threads that are
//! left after the fixed demands are met, so two of them starting together
//! with a budget of 8 get 4 each, and the runner passes the share to the tool
//! as `--threads <share>` (see [`ResolvedStep::command_args`]).
//!
//! The [`Scheduler`] itself does not run anything: the runner asks it which
//! steps to [`launch`](Scheduler::launch), and reports back when one
//! [`finish`](Scheduler::finish)es.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use crate::resolve::ResolvedPlan;
use crate::resolve::ResolvedStep;

/// How many threads a step needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadDemand {
    /// The step uses exactly this many threads.
    Fixed(NonZeroUsize),

    /// The step can use any number of threads, which the runner chooses and
    /// passes as `--threads <n>`.
    Flexible,
}

impl ThreadDemand {
    /// The fewest threads the step can start with.
    fn minimum(self) -> usize {
        match self {
            ThreadDemand::Fixed(threads) => threads.get(),
            ThreadDemand::Flexible => 1,
        }
    }
}

impl ResolvedStep {
    /// How many threads this step needs.
    pub fn thread_demand(&self) -> ThreadDemand {
        if let Some(threads) = self.explicit_threads() {
            return ThreadDemand::Fixed(threads);
        }

        if self.accepts_threads() {
            ThreadDemand::Flexible
        } else {
            ThreadDemand::Fixed(NonZeroUsize::MIN)
        }
    }

    /// The arguments to run this step with when it is given `threads`
    /// threads: [`Self::args`], plus `--threads <threads>` if the step is
    /// [`ThreadDemand::Flexible`].
    pub fn command_args(&self, threads: usize) -> Vec<String> {
        let mut args = self.args.clone();
        if self.thread_demand() == ThreadDemand::Flexible {
            args.push("--threads".to_string());
            args.push(threads.to_string());
        }
        args
    }

    /// The thread count the spec itself passes, as `--threads <n>` or
    /// `--threads=<n>`. A count of 0 is taken as 1, and a value that is not a
    /// number is ignored, since the tool will reject it anyway.
    fn explicit_threads(&self) -> Option<NonZeroUsize> {
        let value = self.args.iter().enumerate().find_map(|(index, arg)| match arg.strip_prefix("--threads") {
            Some("") => self.args.get(index + 1).map(String::as_str),
            Some(rest) => rest.strip_prefix('='),
            None => None,
        })?;

        let threads: usize = value.parse().ok()?;
        Some(NonZeroUsize::new(threads).unwrap_or(NonZeroUsize::MIN))
    }

    /// Whether the runner may choose this step's thread count by passing
    /// `--threads <n>`.
    ///
    /// Not enabled for any tool yet, so every step runs single threaded
    /// unless its args say otherwise. The merc subcommands that already
    /// accept `--threads` are `merc-lps explore-explicit`, `merc-lts combine`
    /// and `merc-pbes explore-explicit` / `solve`.
    fn accepts_threads(&self) -> bool {
        false
    }
}

/// A step the runner should start now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Launch {
    /// The (0-based) index of the step in [`ResolvedPlan::steps`].
    pub step: usize,

    /// The number of threads reserved for the step.
    pub threads: usize,
}

/// Decides which steps of a plan run when, within a thread budget.
///
/// ```
/// # use std::num::NonZeroUsize;
/// # use merc_core::{JobSpec, Scheduler, resolve};
/// # let spec = JobSpec::from_json(r#"{ "schema_version": 2, "steps": [["merc-lts", "$out.a"]] }"#).unwrap();
/// let plan = resolve(&spec).unwrap();
/// let mut scheduler = Scheduler::new(&plan, NonZeroUsize::new(4).unwrap());
/// while !scheduler.is_done() {
///     for launch in scheduler.launch() {
///         // Start plan.steps[launch.step] with launch.threads threads, ...
///         // ... and once it has exited:
///         scheduler.finish(launch.step);
///     }
/// }
/// ```
#[derive(Debug)]
pub struct Scheduler {
    /// The thread budget.
    total: usize,

    /// The threads not reserved by a running step.
    free: usize,

    demands: Vec<ThreadDemand>,

    /// For every step, the steps that depend on it.
    dependents: Vec<Vec<usize>>,

    /// For every step, the number of its dependencies that have not finished.
    waiting_on: Vec<usize>,

    /// The threads reserved by every running step.
    reserved: Vec<Option<usize>>,

    /// The steps whose dependencies have all finished but that have not been
    /// launched, in plan order.
    ready: BTreeSet<usize>,

    /// The number of steps that have finished.
    finished: usize,
}

impl Scheduler {
    pub fn new(plan: &ResolvedPlan, threads: NonZeroUsize) -> Self {
        let count = plan.steps.len();
        let mut dependents = vec![Vec::new(); count];
        for (index, step) in plan.steps.iter().enumerate() {
            for &dependency in &step.depends_on {
                dependents[dependency].push(index);
            }
        }

        Self {
            total: threads.get(),
            free: threads.get(),
            demands: plan.steps.iter().map(ResolvedStep::thread_demand).collect(),
            dependents,
            waiting_on: plan.steps.iter().map(|step| step.depends_on.len()).collect(),
            reserved: vec![None; count],
            ready: (0..count).filter(|&index| plan.steps[index].depends_on.is_empty()).collect(),
            finished: 0,
        }
    }

    /// Reserves threads for, and returns, the steps that can start now.
    ///
    /// Ready steps are considered in plan order, and each one starts if its
    /// minimum demand fits in the free threads; a step that does not fit does
    /// not hold back later, smaller ones. A fixed demand larger than the whole
    /// budget is capped to it, so such a step runs once nothing else does.
    /// The threads left over after that are divided evenly over the flexible
    /// steps that start, earlier steps getting the remainder.
    pub fn launch(&mut self) -> Vec<Launch> {
        let mut launches = Vec::new();
        let mut flexible = 0;
        for &step in &self.ready {
            let needed = self.demands[step].minimum().min(self.total);
            if needed <= self.free {
                self.free -= needed;
                launches.push(Launch { step, threads: needed });
                if self.demands[step] == ThreadDemand::Flexible {
                    flexible += 1;
                }
            }
        }

        if let Some(share) = self.free.checked_div(flexible) {
            let mut remainder = self.free % flexible;
            for launch in &mut launches {
                if self.demands[launch.step] == ThreadDemand::Flexible {
                    let extra = share + usize::from(remainder > 0);
                    remainder = remainder.saturating_sub(1);
                    launch.threads += extra;
                    self.free -= extra;
                }
            }
        }

        for launch in &launches {
            self.ready.remove(&launch.step);
            self.reserved[launch.step] = Some(launch.threads);
        }

        launches
    }

    /// Records that `step` has finished successfully, releasing its threads
    /// and making the steps that only waited for it ready.
    ///
    /// # Panics
    ///
    /// If `step` is not running.
    pub fn finish(&mut self, step: usize) {
        let threads = self.reserved[step].take().expect("finished step was not running");
        self.free += threads;
        self.finished += 1;

        for &dependent in &self.dependents[step] {
            self.waiting_on[dependent] -= 1;
            if self.waiting_on[dependent] == 0 {
                self.ready.insert(dependent);
            }
        }
    }

    /// Whether every step has finished.
    pub fn is_done(&self) -> bool {
        self.finished == self.demands.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Tool;

    fn step(depends_on: &[usize], args: &[&str]) -> ResolvedStep {
        ResolvedStep {
            tool: Tool::MercLts,
            args: args.iter().map(|arg| arg.to_string()).collect(),
            outputs: Vec::new(),
            depends_on: depends_on.to_vec(),
        }
    }

    fn threads(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap()
    }

    fn scheduler(steps: Vec<ResolvedStep>, total: usize) -> Scheduler {
        Scheduler::new(&ResolvedPlan { steps, keep: Vec::new() }, threads(total))
    }

    fn launched(launches: &[Launch]) -> Vec<(usize, usize)> {
        launches.iter().map(|launch| (launch.step, launch.threads)).collect()
    }

    #[test]
    fn reads_explicit_thread_counts() {
        assert_eq!(step(&[], &["combine"]).thread_demand(), ThreadDemand::Fixed(threads(1)));
        assert_eq!(step(&[], &["combine", "--threads", "3"]).thread_demand(), ThreadDemand::Fixed(threads(3)));
        assert_eq!(step(&[], &["combine", "--threads=5"]).thread_demand(), ThreadDemand::Fixed(threads(5)));
        assert_eq!(step(&[], &["combine", "--threads", "0"]).thread_demand(), ThreadDemand::Fixed(threads(1)));
        assert_eq!(step(&[], &["combine", "--threads-x", "4"]).thread_demand(), ThreadDemand::Fixed(threads(1)));
    }

    #[test]
    fn runs_independent_steps_in_parallel_within_the_budget() {
        // Three independent steps and one that joins them, with two threads.
        let mut scheduler = scheduler(vec![step(&[], &[]), step(&[], &[]), step(&[], &[]), step(&[0, 1, 2], &[])], 2);

        assert_eq!(launched(&scheduler.launch()), vec![(0, 1), (1, 1)]);
        assert!(scheduler.launch().is_empty());

        scheduler.finish(1);
        assert_eq!(launched(&scheduler.launch()), vec![(2, 1)]);

        scheduler.finish(0);
        assert!(scheduler.launch().is_empty(), "step 3 still waits for step 2");

        scheduler.finish(2);
        assert_eq!(launched(&scheduler.launch()), vec![(3, 1)]);

        scheduler.finish(3);
        assert!(scheduler.is_done());
    }

    #[test]
    fn a_large_step_does_not_hold_back_smaller_ones() {
        let mut scheduler = scheduler(vec![step(&[], &[]), step(&[], &["--threads", "4"]), step(&[], &[])], 4);

        assert_eq!(launched(&scheduler.launch()), vec![(0, 1), (2, 1)]);

        scheduler.finish(0);
        scheduler.finish(2);
        assert_eq!(launched(&scheduler.launch()), vec![(1, 4)]);
    }

    #[test]
    fn caps_demands_larger_than_the_budget() {
        let mut scheduler = scheduler(vec![step(&[], &["--threads", "16"])], 4);
        assert_eq!(launched(&scheduler.launch()), vec![(0, 4)]);
    }

    #[test]
    fn divides_free_threads_over_flexible_steps() {
        let mut scheduler = scheduler(vec![step(&[], &[]), step(&[], &[]), step(&[], &[])], 8);
        scheduler.demands = vec![ThreadDemand::Flexible, ThreadDemand::Fixed(threads(1)), ThreadDemand::Flexible];

        // One thread for the fixed step, the other seven split 4 + 3.
        assert_eq!(launched(&scheduler.launch()), vec![(0, 4), (1, 1), (2, 3)]);
        assert_eq!(scheduler.free, 0);
    }

    #[test]
    fn a_flexible_step_alone_gets_every_free_thread() {
        let mut scheduler = scheduler(vec![step(&[], &[]), step(&[], &[]), step(&[0], &[])], 8);
        scheduler.demands[2] = ThreadDemand::Flexible;

        assert_eq!(launched(&scheduler.launch()), vec![(0, 1), (1, 1)]);
        scheduler.finish(0);
        assert_eq!(launched(&scheduler.launch()), vec![(2, 7)]);
    }
}
