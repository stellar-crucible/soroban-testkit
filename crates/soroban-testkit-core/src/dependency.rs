//! Contract call dependency graphs.
//!
//! A [`DependencyGraph`] records which contract invoked which during a test and
//! answers the questions a micro-contract ecosystem raises: can these calls be
//! ordered? Did a call loop back — directly, indirectly, or into itself?
//!
//! The graph is built from the Soroban host's `fn_call` diagnostic events, which
//! carry the invoking contract, the invoked contract and the function name, so
//! no instrumentation of the contracts is needed:
//!
//! ```rust
//! use soroban_testkit_core::DependencyGraph;
//! # use soroban_sdk::Env;
//!
//! # fn example(env: &Env) {
//! // After the invocation under test:
//! let graph = DependencyGraph::from_env(env);
//!
//! println!("{}", graph.to_mermaid());
//! if let Some(cycle) = graph.find_cycle() {
//!     panic!("contracts call each other in a loop: {cycle}");
//! }
//! # }
//! ```
//!
//! [`DependencyGraph::record`] accumulates several invocations into one graph,
//! because the host reports diagnostics for the most recent invocation only.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use soroban_sdk::{
    xdr::{ContractEventBody, ContractEventType, ContractId, Hash, ScAddress, ScVal},
    Address, Env, TryFromVal,
};

/// One directed edge of a dependency graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEdge {
    /// The contract that made the call.
    pub caller: String,
    /// The contract that was called.
    pub callee: String,
    /// How many times `caller` invoked `callee`.
    pub calls: usize,
    /// The contract functions invoked across those calls, sorted.
    pub functions: Vec<String>,
}

impl DependencyEdge {
    fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.functions.is_empty() {
            parts.push(self.functions.join(", "));
        }
        if self.calls > 1 {
            parts.push(format!("x{}", self.calls));
        }
        parts.join(" ")
    }
}

/// A closed chain of contract calls, e.g. `A -> B -> C -> A`.
///
/// The first and last entries are the same contract. A self-recursive call is
/// the two-entry path `A -> A`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyCycle {
    path: Vec<String>,
}

impl DependencyCycle {
    fn new(path: Vec<String>) -> Self {
        Self { path }
    }

    /// The contracts along the cycle, with the starting contract repeated last.
    pub fn path(&self) -> &[String] {
        &self.path
    }

    /// True when a contract called itself.
    pub fn is_self_recursive(&self) -> bool {
        self.path.len() == 2
    }

    /// The number of distinct contracts on the cycle.
    pub fn len(&self) -> usize {
        self.path.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Display for DependencyCycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.path.join(" -> "))
    }
}

impl std::error::Error for DependencyCycle {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct EdgeStats {
    calls: usize,
    functions: BTreeSet<String>,
}

/// A directed graph of contract-to-contract invocations.
///
/// Nodes are contract addresses rendered as strkeys (`C…`). Edges point from
/// the invoking contract to the invoked contract and remember how often and
/// through which functions the call happened.
///
/// Build one from a test invocation with [`DependencyGraph::from_env`], or
/// accumulate several invocations with [`DependencyGraph::record`]. Edges can
/// also be added by hand for graphs assembled from a specification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyGraph {
    nodes: BTreeSet<String>,
    edges: BTreeMap<(String, String), EdgeStats>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a graph from the contract invocations of the most recent
    /// top-level call in `env`.
    ///
    /// This includes every nested contract call, not only the top-level one.
    /// The top-level entry point becomes a node; it has no incoming edge
    /// because the test harness, not a contract, invoked it.
    pub fn from_env(env: &Env) -> Self {
        let mut graph = Self::new();
        graph.record(env);
        graph
    }

    /// Add the contract invocations of the most recent top-level call to this
    /// graph, keeping everything recorded before it.
    ///
    /// Call after each invocation a test means to judge, as with `EventLog`.
    /// Recording the same invocation twice counts its calls twice.
    pub fn record(&mut self, env: &Env) -> &mut Self {
        let events = match env.host().get_diagnostic_events() {
            Ok(events) => events.0,
            Err(_) => return self,
        };

        for host_event in events {
            let event = host_event.event;
            if event.type_ != ContractEventType::Diagnostic {
                continue;
            }
            let ContractEventBody::V0(body) = event.body;
            let topics = &body.topics;
            let (
                Some(ScVal::Symbol(kind)),
                Some(ScVal::Bytes(callee)),
                Some(ScVal::Symbol(function)),
            ) = (topics.first(), topics.get(1), topics.get(2))
            else {
                continue;
            };
            if kind.0.as_slice() != b"fn_call" {
                continue;
            }
            let Ok(hash) = <[u8; 32]>::try_from(callee.0.as_slice()) else {
                continue;
            };
            let callee = address_label(env, hash);
            self.nodes.insert(callee.clone());

            if let Some(caller) = &event.contract_id {
                let caller = address_label(env, (caller.0).0);
                self.nodes.insert(caller.clone());
                self.add_call(&caller, &callee, &String::from_utf8_lossy(&function.0));
            }
        }

        self
    }

    /// Add one directed call from `caller` to `callee`.
    pub fn add_edge(&mut self, caller: impl AsRef<str>, callee: impl AsRef<str>) -> &mut Self {
        self.add_call(caller.as_ref(), callee.as_ref(), "");
        self
    }

    /// Add one directed call between two contract addresses.
    pub fn add_contract_edge(&mut self, caller: &Address, callee: &Address) -> &mut Self {
        self.add_call(&strkey(caller), &strkey(callee), "");
        self
    }

    /// Add a contract as a node even if it has no edges yet.
    pub fn add_node(&mut self, contract: impl AsRef<str>) -> &mut Self {
        self.nodes.insert(contract.as_ref().to_string());
        self
    }

    fn add_call(&mut self, caller: &str, callee: &str, function: &str) {
        self.nodes.insert(caller.to_string());
        self.nodes.insert(callee.to_string());
        let stats = self
            .edges
            .entry((caller.to_string(), callee.to_string()))
            .or_default();
        stats.calls += 1;
        if !function.is_empty() {
            stats.functions.insert(function.to_string());
        }
    }

    /// Every contract in the graph, sorted.
    pub fn nodes(&self) -> Vec<&str> {
        self.nodes.iter().map(String::as_str).collect()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }

    /// Every edge, ordered by caller and then callee.
    pub fn edges(&self) -> Vec<DependencyEdge> {
        self.edges
            .iter()
            .map(|((caller, callee), stats)| DependencyEdge {
                caller: caller.clone(),
                callee: callee.clone(),
                calls: stats.calls,
                functions: stats.functions.iter().cloned().collect(),
            })
            .collect()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// The edge from `caller` to `callee`, if the call happened.
    pub fn edge(&self, caller: &str, callee: &str) -> Option<DependencyEdge> {
        self.edges
            .get(&(caller.to_string(), callee.to_string()))
            .map(|stats| DependencyEdge {
                caller: caller.to_string(),
                callee: callee.to_string(),
                calls: stats.calls,
                functions: stats.functions.iter().cloned().collect(),
            })
    }

    pub fn has_edge(&self, caller: &str, callee: &str) -> bool {
        self.edges
            .contains_key(&(caller.to_string(), callee.to_string()))
    }

    /// Order the contracts so every caller comes before the contracts it calls.
    ///
    /// Isolated contracts are included. The order is deterministic: whenever
    /// several contracts are ready, the lexicographically smallest goes first.
    ///
    /// # Errors
    ///
    /// Fails with one concrete cycle when the graph cannot be ordered.
    pub fn topological_sort(&self) -> Result<Vec<String>, DependencyCycle> {
        let adjacency = self.adjacency();
        let mut indegree: BTreeMap<&str, usize> =
            self.nodes.iter().map(|node| (node.as_str(), 0)).collect();
        for callee in self.edges.keys().map(|(_, callee)| callee) {
            if let Some(count) = indegree.get_mut(callee.as_str()) {
                *count += 1;
            }
        }

        let mut ready: BTreeSet<&str> = indegree
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(node, _)| *node)
            .collect();
        let mut order: Vec<String> = Vec::with_capacity(self.nodes.len());

        while let Some(node) = ready.pop_first() {
            order.push(node.to_string());
            for next in adjacency.get(node).into_iter().flatten() {
                let count = indegree
                    .get_mut(next.as_str())
                    .expect("every edge target is a node");
                *count -= 1;
                if *count == 0 {
                    ready.insert(next);
                }
            }
        }

        if order.len() == self.nodes.len() {
            Ok(order)
        } else {
            Err(self
                .find_cycle()
                .expect("a graph with no topological order contains a cycle"))
        }
    }

    /// True when at least one contract call chain loops back on itself.
    pub fn has_cycle(&self) -> bool {
        self.find_cycle().is_some()
    }

    /// The first cycle found, walking contracts in sorted order.
    pub fn find_cycle(&self) -> Option<DependencyCycle> {
        let adjacency = self.adjacency();
        let mut state: BTreeMap<String, u8> = BTreeMap::new();
        let mut stack: Vec<String> = Vec::new();

        for node in &self.nodes {
            if state.get(node).copied().unwrap_or(0) == 0 {
                if let Some(path) = walk_for_cycle(node, &adjacency, &mut state, &mut stack) {
                    return Some(DependencyCycle::new(path));
                }
            }
        }
        None
    }

    /// One representative cycle per strongly connected component that can loop.
    ///
    /// A component of two or more contracts is cyclic when every member can
    /// reach every other member. A single contract is cyclic only when it calls
    /// itself.
    pub fn cycles(&self) -> Vec<DependencyCycle> {
        let adjacency = self.adjacency();
        let mut tarjan = Tarjan::default();
        for node in &self.nodes {
            if !tarjan.indices.contains_key(node) {
                tarjan.visit(node, &adjacency);
            }
        }

        let mut cycles: Vec<DependencyCycle> = Vec::new();
        for component in tarjan.components {
            let cyclic = component.len() > 1
                || adjacency
                    .get(&component[0])
                    .is_some_and(|next| next.contains(&component[0]));
            if cyclic {
                cycles.push(DependencyCycle::new(cycle_through(&component, &adjacency)));
            }
        }

        cycles.sort_by(|left, right| left.path.cmp(&right.path));
        cycles
    }

    /// Panic unless the graph has no cycle.
    pub fn assert_acyclic(&self) {
        if let Some(cycle) = self.find_cycle() {
            panic!("Expected an acyclic contract dependency graph, but found a cycle: {cycle}");
        }
    }

    /// Render the graph as Graphviz DOT.
    pub fn to_dot(&self) -> String {
        let mut out = String::from("digraph contract_dependencies {\n");
        out.push_str("  rankdir=LR;\n");
        out.push_str("  node [shape=box, style=rounded, fontname=\"monospace\"];\n");
        for node in &self.nodes {
            out.push_str(&format!(
                "  \"{}\" [label=\"{}\"];\n",
                escape_dot(node),
                escape_dot(node),
            ));
        }
        for edge in self.edges() {
            let label = edge.label();
            if label.is_empty() {
                out.push_str(&format!(
                    "  \"{}\" -> \"{}\";\n",
                    escape_dot(&edge.caller),
                    escape_dot(&edge.callee),
                ));
            } else {
                out.push_str(&format!(
                    "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
                    escape_dot(&edge.caller),
                    escape_dot(&edge.callee),
                    escape_dot(&label),
                ));
            }
        }
        out.push_str("}\n");
        out
    }

    /// Render the graph as a Mermaid `flowchart`.
    pub fn to_mermaid(&self) -> String {
        let ids: BTreeMap<&str, String> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.as_str(), format!("n{index}")))
            .collect();

        let mut out = String::from("flowchart LR\n");
        for (node, id) in &ids {
            out.push_str(&format!("  {id}[\"{}\"]\n", escape_mermaid(node)));
        }
        for edge in self.edges() {
            let from = &ids[edge.caller.as_str()];
            let to = &ids[edge.callee.as_str()];
            let label = edge.label();
            if label.is_empty() {
                out.push_str(&format!("  {from} --> {to}\n"));
            } else {
                out.push_str(&format!("  {from} -->|{}| {to}\n", escape_mermaid(&label)));
            }
        }
        out
    }

    fn adjacency(&self) -> BTreeMap<String, BTreeSet<String>> {
        let mut adjacency: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for node in &self.nodes {
            adjacency.entry(node.clone()).or_default();
        }
        for (caller, callee) in self.edges.keys() {
            adjacency
                .entry(caller.clone())
                .or_default()
                .insert(callee.clone());
        }
        adjacency
    }
}

fn strkey(address: &Address) -> String {
    format!("{}", address.to_string())
}

fn address_label(env: &Env, hash: [u8; 32]) -> String {
    let address = ScAddress::Contract(ContractId(Hash(hash)));
    Address::try_from_val(env, &address)
        .map(|address| strkey(&address))
        .unwrap_or_else(|_| {
            hash.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join("")
        })
}

fn walk_for_cycle(
    node: &str,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    state: &mut BTreeMap<String, u8>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    state.insert(node.to_string(), 1);
    stack.push(node.to_string());

    for next in adjacency.get(node).into_iter().flatten() {
        match state.get(next).copied().unwrap_or(0) {
            0 => {
                if let Some(cycle) = walk_for_cycle(next, adjacency, state, stack) {
                    return Some(cycle);
                }
            }
            1 => {
                let start = stack
                    .iter()
                    .position(|visited| visited == next)
                    .expect("a visiting node is on the stack");
                let mut path = stack[start..].to_vec();
                path.push(next.clone());
                return Some(path);
            }
            _ => {}
        }
    }

    stack.pop();
    state.insert(node.to_string(), 2);
    None
}

#[derive(Default)]
struct Tarjan {
    index: usize,
    indices: BTreeMap<String, usize>,
    lowlink: BTreeMap<String, usize>,
    on_stack: BTreeSet<String>,
    stack: Vec<String>,
    components: Vec<Vec<String>>,
}

impl Tarjan {
    fn visit(&mut self, node: &str, adjacency: &BTreeMap<String, BTreeSet<String>>) {
        let index = self.index;
        self.indices.insert(node.to_string(), index);
        self.lowlink.insert(node.to_string(), index);
        self.index += 1;
        self.stack.push(node.to_string());
        self.on_stack.insert(node.to_string());

        for next in adjacency.get(node).into_iter().flatten() {
            if !self.indices.contains_key(next) {
                self.visit(next, adjacency);
                let low = self.lowlink[next];
                let current = self.lowlink[node];
                self.lowlink.insert(node.to_string(), current.min(low));
            } else if self.on_stack.contains(next) {
                let next_index = self.indices[next];
                let current = self.lowlink[node];
                self.lowlink
                    .insert(node.to_string(), current.min(next_index));
            }
        }

        if self.lowlink[node] == self.indices[node] {
            let mut component = Vec::new();
            while let Some(member) = self.stack.pop() {
                self.on_stack.remove(&member);
                let done = member == node;
                component.push(member);
                if done {
                    break;
                }
            }
            component.sort();
            self.components.push(component);
        }
    }
}

fn cycle_through(
    component: &[String],
    adjacency: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    let start = &component[0];
    if component.len() == 1 {
        return vec![start.clone(), start.clone()];
    }

    let members: BTreeSet<&str> = component.iter().map(String::as_str).collect();
    let mut path = vec![start.clone()];
    let mut visited: BTreeSet<String> = BTreeSet::new();
    visited.insert(start.clone());

    if path_back(start, start, adjacency, &members, &mut visited, &mut path) {
        path
    } else {
        // A strongly connected component always has a path back to its start;
        // keep the members rather than inventing an edge if that ever changes.
        let mut fallback = component.to_vec();
        fallback.push(start.clone());
        fallback
    }
}

fn path_back(
    start: &str,
    current: &str,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    members: &BTreeSet<&str>,
    visited: &mut BTreeSet<String>,
    path: &mut Vec<String>,
) -> bool {
    for next in adjacency.get(current).into_iter().flatten() {
        if !members.contains(next.as_str()) {
            continue;
        }
        if next == start {
            path.push(start.to_string());
            return true;
        }
        if visited.insert(next.clone()) {
            path.push(next.clone());
            if path_back(start, next, adjacency, members, visited, path) {
                return true;
            }
            path.pop();
            visited.remove(next);
        }
    }
    false
}

fn escape_dot(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_mermaid(text: &str) -> String {
    text.replace('"', "'")
        .replace('|', "/")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::{strkey, DependencyCycle, DependencyGraph};
    use soroban_sdk::{contract, contractimpl, Address, Env};

    #[contract]
    pub struct ContractA;

    #[contractimpl]
    impl ContractA {
        pub fn call_b(env: Env, b: Address) {
            ContractBClient::new(&env, &b).ping();
        }

        pub fn call_chain(env: Env, b: Address, c: Address) {
            ContractBClient::new(&env, &b).ping_c(&c);
        }

        pub fn stop(_env: Env) {}
    }

    #[contract]
    pub struct ContractB;

    #[contractimpl]
    impl ContractB {
        pub fn ping(_env: Env) {}

        pub fn ping_c(env: Env, c: Address) {
            ContractCClient::new(&env, &c).stop();
        }
    }

    #[contract]
    pub struct ContractC;

    #[contractimpl]
    impl ContractC {
        pub fn stop(_env: Env) {}
    }

    fn registered() -> (Env, Address, Address, Address) {
        let env = Env::default();
        let a = env.register(ContractA, ());
        let b = env.register(ContractB, ());
        let c = env.register(ContractC, ());
        (env, a, b, c)
    }

    #[test]
    fn records_nested_contract_calls() {
        let (env, a, b, c) = registered();
        ContractAClient::new(&env, &a).call_chain(&b, &c);

        let graph = DependencyGraph::from_env(&env);
        let a = strkey(&a);
        let b = strkey(&b);
        let c = strkey(&c);

        assert_eq!(
            graph
                .nodes()
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            [a.as_str(), b.as_str(), c.as_str()].into_iter().collect(),
        );
        assert_eq!(graph.edge_count(), 2);
        assert!(graph.has_edge(&a, &b));
        assert!(graph.has_edge(&b, &c));
        assert!(!graph.has_edge(&a, &c));

        let edge = graph.edge(&a, &b).unwrap();
        assert_eq!(edge.calls, 1);
        assert_eq!(edge.functions, vec!["ping_c"]);
        assert_eq!(graph.edge(&b, &c).unwrap().functions, vec!["stop"]);

        // The chain is a DAG, so the recorded order is a valid topological one.
        let order = graph.topological_sort().unwrap();
        let position = |node: &str| order.iter().position(|entry| entry == node).unwrap();
        assert!(position(&a) < position(&b));
        assert!(position(&b) < position(&c));
    }

    #[test]
    fn detects_a_direct_cycle_between_two_contracts() {
        let mut graph = DependencyGraph::new();
        graph.add_edge("A", "B").add_edge("B", "A");

        assert!(graph.has_cycle());
        let cycle = graph.find_cycle().unwrap();
        assert_eq!(cycle.path(), &["A", "B", "A"]);
        assert_eq!(cycle.len(), 2);
        assert!(!cycle.is_self_recursive());
        assert!(graph.topological_sort().is_err());
    }

    #[test]
    fn detects_an_indirect_cycle_across_three_contracts() {
        let mut graph = DependencyGraph::new();
        graph.add_edge("A", "B");
        graph.add_edge("B", "C");
        graph.add_edge("C", "A");

        assert!(graph.has_cycle());

        let cycle = graph.find_cycle().unwrap();
        assert_eq!(cycle.path(), &["A", "B", "C", "A"]);

        let cycles = graph.cycles();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].path(), cycle.path());
    }

    #[test]
    fn record_accumulates_across_invocations() {
        let (env, a, b, _) = registered();
        let client = ContractAClient::new(&env, &a);
        let mut graph = DependencyGraph::new();

        client.call_b(&b);
        graph.record(&env);
        client.call_b(&b);
        graph.record(&env);

        let a = strkey(&a);
        let b = strkey(&b);
        let edge = graph.edge(&a, &b).unwrap();
        assert_eq!(edge.calls, 2);
        assert_eq!(graph.nodes().len(), 2);
    }

    #[test]
    fn from_env_reads_only_the_latest_invocation() {
        let (env, a, b, _) = registered();
        let client = ContractAClient::new(&env, &a);

        client.call_b(&b);
        client.stop();

        let graph = DependencyGraph::from_env(&env);
        let a = strkey(&a);
        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.nodes(), vec![a.as_str()]);
        assert!(graph.edges().is_empty());
    }

    #[test]
    fn a_dag_topologically_sorts() {
        let mut graph = DependencyGraph::new();
        graph.add_edge("C", "A");
        graph.add_edge("B", "A");
        graph.add_edge("A", "D");

        let order: Vec<String> = graph.topological_sort().unwrap();
        assert_eq!(order, vec!["B", "C", "A", "D"]);
        graph.assert_acyclic();
        assert!(graph.find_cycle().is_none());
        assert!(graph.cycles().is_empty());
    }

    #[test]
    fn a_self_call_is_detected_as_recursion() {
        let mut graph = DependencyGraph::new();
        graph.add_edge("A", "A");

        let cycle = graph.find_cycle().unwrap();
        assert!(cycle.is_self_recursive());
        assert_eq!(cycle.path(), &["A", "A"]);
        assert_eq!(graph.cycles().len(), 1);
        assert!(graph.topological_sort().is_err());
    }

    #[test]
    #[should_panic(expected = "found a cycle: A -> B -> A")]
    fn assert_acyclic_names_the_cycle() {
        let mut graph = DependencyGraph::new();
        graph.add_edge("A", "B");
        graph.add_edge("B", "A");

        graph.assert_acyclic();
    }

    #[test]
    fn topological_sort_reports_the_cycle_it_found() {
        let mut graph = DependencyGraph::new();
        graph.add_edge("A", "B");
        graph.add_edge("B", "C");
        graph.add_edge("C", "A");

        let error: DependencyCycle = graph.topological_sort().unwrap_err();
        assert_eq!(error.to_string(), "A -> B -> C -> A");
    }

    #[test]
    fn an_empty_graph_is_acyclic_and_empty_in_every_format() {
        let graph = DependencyGraph::new();

        assert!(graph.is_empty());
        assert!(graph.topological_sort().unwrap().is_empty());
        assert!(!graph.has_cycle());
        assert_eq!(
            graph.to_dot(),
            "digraph contract_dependencies {\n  rankdir=LR;\n  node [shape=box, style=rounded, fontname=\"monospace\"];\n}\n"
        );
        assert_eq!(graph.to_mermaid(), "flowchart LR\n");
    }

    #[test]
    fn dot_export_is_valid_graphviz() {
        let mut graph = DependencyGraph::new();
        graph.add_edge("A", "B").add_edge("B", "C");

        let dot = graph.to_dot();
        assert!(dot.starts_with("digraph contract_dependencies {\n"));
        assert!(dot.trim_end().ends_with('}'));
        assert!(dot.contains("  \"A\" [label=\"A\"];\n"));
        assert!(dot.contains("  \"A\" -> \"B\";\n"));
        assert!(dot.contains("  \"B\" -> \"C\";\n"));
        assert_eq!(dot.matches("->").count(), 2);
    }

    #[test]
    fn mermaid_export_is_valid_flowchart() {
        let mut graph = DependencyGraph::new();
        graph.add_edge("A", "B");
        graph.add_edge("B", "A");

        let mermaid = graph.to_mermaid();
        let lines: Vec<&str> = mermaid.lines().collect();
        assert_eq!(lines[0], "flowchart LR");
        assert_eq!(lines[1], "  n0[\"A\"]");
        assert_eq!(lines[2], "  n1[\"B\"]");
        assert_eq!(lines[3], "  n0 --> n1");
        assert_eq!(lines[4], "  n1 --> n0");
    }

    #[test]
    fn renderers_include_call_counts_and_escaped_labels() {
        let mut graph = DependencyGraph::new();
        graph.add_edge("A", "B");
        graph.add_call("A", "B", "ping");
        graph.add_edge("A", "A");

        let dot = graph.to_dot();
        assert!(dot.contains("\"A\" -> \"B\" [label=\"ping x2\"];"));

        let mermaid = graph.to_mermaid();
        assert!(mermaid.contains("-->|ping x2|"));

        let mut quoted = DependencyGraph::new();
        quoted.add_edge("say \"hi\"", "b|c");
        assert!(quoted.to_dot().contains("\\\"hi\\\""));
        assert!(quoted.to_mermaid().contains("say 'hi'"));
        assert!(quoted.to_mermaid().contains("b/c"));
    }

    #[test]
    fn isolated_nodes_are_sorted_and_kept() {
        let mut graph = DependencyGraph::new();
        graph.add_node("Z");
        graph.add_node("A");
        graph.add_edge("B", "A");

        assert_eq!(graph.nodes(), vec!["A", "B", "Z"]);
        assert_eq!(graph.topological_sort().unwrap(), vec!["B", "A", "Z"]);
    }

    #[test]
    fn contract_edges_use_strkey_labels() {
        let (_env, a, b, _) = registered();
        let mut graph = DependencyGraph::new();
        graph.add_contract_edge(&a, &b);

        let a = strkey(&a);
        let b = strkey(&b);
        assert!(graph.has_edge(&a, &b));
        assert!(graph.to_mermaid().contains(&a));
    }
}
