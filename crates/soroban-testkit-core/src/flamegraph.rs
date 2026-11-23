//! CPU instruction flamegraphs for Soroban contract call frames.
//!
//! The host metering already recovered by [`CallTracer`](crate::tracer::CallTracer)
//! contains a call tree for the last top-level invocation.  This module folds
//! that tree into inferno-compatible folded stacks and renders a small standalone
//! SVG or terminal hotspot summary.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use soroban_sdk::Env;

use crate::tracer::CallTracer;

/// A function or call-frame path with CPU instructions attributed to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlamegraphHotspot {
    pub frame: String,
    pub cpu_insns: u64,
}

/// Collects and renders folded CPU instruction stacks.
#[derive(Debug, Clone, Default)]
pub struct GasFlamegraphGenerator {
    stacks: BTreeMap<Vec<String>, u64>,
    labels: BTreeMap<String, String>,
}

impl GasFlamegraphGenerator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a human-readable label for an address or frame name.
    pub fn label(mut self, address: impl Into<String>, label: impl Into<String>) -> Self {
        self.labels.insert(address.into(), label.into());
        self
    }

    /// Run an invocation and record its per-function call frames.
    pub fn capture<T>(&mut self, env: &Env, invocation: impl FnOnce() -> T) -> Option<T> {
        let mut tracer = CallTracer::new();
        let value = tracer.record(env, invocation);
        self.record_tracer(&tracer);
        value
    }

    /// Fold a previously captured [`CallTracer`] into this generator.
    pub fn record_tracer(&mut self, tracer: &CallTracer) {
        for root in tracer.roots() {
            let mut path = Vec::new();
            self.record_frame(root, &mut path);
        }
    }

    fn record_frame(&mut self, frame: &crate::tracer::TraceFrame, path: &mut Vec<String>) {
        let contract = format!("{:?}", frame.contract);
        let contract_name = self.labels.get(&contract).cloned().unwrap_or(contract);
        path.push(format!("{contract_name}::{}", frame.function));
        let cpu = frame.self_cpu_insns();
        if cpu > 0 {
            *self.stacks.entry(path.clone()).or_insert(0) += cpu;
        }
        for child in &frame.children {
            self.record_frame(child, path);
        }
        path.pop();
    }

    /// Add a pre-built folded stack, for hosts or profilers that supply frames.
    pub fn record_stack<I, S>(&mut self, frames: I, cpu_insns: u64)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let path: Vec<String> = frames.into_iter().map(Into::into).collect();
        if !path.is_empty() && cpu_insns > 0 {
            *self.stacks.entry(path).or_insert(0) += cpu_insns;
        }
    }

    /// Inferno-compatible folded stack text.
    pub fn to_folded(&self) -> String {
        let mut lines = Vec::new();
        for (path, cpu) in &self.stacks {
            lines.push(format!("{} {}", path.join(";"), cpu));
        }
        lines.join("\n")
    }

    /// Alias useful to integrations that write `.folded` artifacts.
    pub fn export_folded(&self) -> String {
        self.to_folded()
    }

    /// Highest self-CPU call-frame paths, largest first.
    pub fn hotspots(&self, limit: usize) -> Vec<FlamegraphHotspot> {
        let mut items: Vec<_> = self
            .stacks
            .iter()
            .map(|(path, cpu)| FlamegraphHotspot {
                frame: path.last().cloned().unwrap_or_default(),
                cpu_insns: *cpu,
            })
            .collect();
        items.sort_by(|a, b| {
            b.cpu_insns
                .cmp(&a.cpu_insns)
                .then_with(|| a.frame.cmp(&b.frame))
        });
        items.truncate(limit);
        items
    }

    /// Terminal-renderable summary for CI logs.
    pub fn terminal_summary(&self, limit: usize) -> String {
        let mut summary = String::from("CPU flamegraph hotspots:\n");
        let hotspots = self.hotspots(limit);
        if hotspots.is_empty() {
            return String::from("CPU flamegraph hotspots: no samples");
        }
        let max = hotspots.first().map_or(1, |item| item.cpu_insns);
        for item in hotspots {
            let bars = if max == 0 {
                0
            } else {
                ((item.cpu_insns as f64 / max as f64) * 24.0).ceil() as usize
            };
            let _ = writeln!(
                summary,
                "{:>12} insns | {:<24} | {}",
                item.cpu_insns,
                "#".repeat(bars.max(1)),
                item.frame
            );
        }
        summary
    }

    /// Standalone SVG flamegraph.  The output has no external dependencies.
    pub fn to_svg(&self) -> String {
        let mut samples: Vec<_> = self.stacks.iter().collect();
        samples.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let max = samples.first().map_or(1, |(_, cpu)| **cpu).max(1);
        let width = 1000u64;
        let height = (samples.len() as u64 * 22 + 40).max(62);
        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1000\" height=\"{height}\">\n\
             <rect width=\"100%\" height=\"100%\" fill=\"#111827\"/><text x=\"12\" y=\"24\" fill=\"#fff\" font-family=\"sans-serif\" font-size=\"16\">CPU instruction flamegraph</text>\n"
        );
        for (index, (path, cpu)) in samples.iter().enumerate() {
            let bar_width = ((**cpu as f64 / max as f64) * width as f64).ceil() as u64;
            let y = 36 + index as u64 * 22;
            let name = xml_escape(path.last().map_or("", String::as_str));
            let _ = writeln!(
                svg,
                "<rect x=\"10\" y=\"{y}\" width=\"{bar_width}\" height=\"18\" fill=\"#ef4444\"><title>{} instructions</title></rect><text x=\"16\" y=\"{}\" fill=\"#fff\" font-family=\"monospace\" font-size=\"12\">{name} ({} insns)</text>",
                cpu,
                y + 13,
                cpu
            );
        }
        svg.push_str("</svg>\n");
        svg
    }

    pub fn is_empty(&self) -> bool {
        self.stacks.is_empty()
    }

    pub fn total_cpu_insns(&self) -> u64 {
        self.stacks.values().copied().sum()
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folded_stack_attributes_cpu_to_function_frames() {
        let mut generator = GasFlamegraphGenerator::new();
        generator.record_stack(["pool::swap", "pool::cross_tick"], 125);
        generator.record_stack(["pool::swap", "pool::fee"], 75);

        let folded = generator.export_folded();
        assert!(folded.contains("pool::swap;pool::cross_tick 125"));
        assert!(folded.contains("pool::swap;pool::fee 75"));
        assert_eq!(generator.total_cpu_insns(), 200);
    }

    #[test]
    fn terminal_and_svg_render_hotspots() {
        let mut generator = GasFlamegraphGenerator::new();
        generator.record_stack(["pool::swap", "pool::cross_tick"], 900);
        generator.record_stack(["pool::swap", "pool::fee"], 100);

        let terminal = generator.terminal_summary(5);
        assert!(terminal.contains("CPU flamegraph hotspots"));
        assert!(terminal.contains("900 insns"));
        assert!(terminal.contains("cross_tick"));

        let svg = generator.to_svg();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("CPU instruction flamegraph"));
        assert!(svg.contains("900 insns"));
        assert!(svg.ends_with("</svg>\n"));
    }

    #[test]
    fn hotspot_limit_and_empty_summary() {
        let mut generator = GasFlamegraphGenerator::new();
        assert_eq!(generator.hotspots(10).len(), 0);
        assert_eq!(
            generator.terminal_summary(10),
            "CPU flamegraph hotspots: no samples"
        );

        generator.record_stack(["first"], 10);
        generator.record_stack(["second"], 30);
        generator.record_stack(["third"], 20);
        let top = generator.hotspots(2);
        assert_eq!(top[0].frame, "second");
        assert_eq!(top[1].frame, "third");
    }
}
