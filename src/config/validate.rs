//! Semantic validation rules for [`ForgeConfig`].
//!
//! Each rule is a small function.  [`validate`] runs them all and
//! reports the first failure.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{
    config::{API_VERSION, ForgeConfig, HealthCheck, KIND, NetworkMode, RuntimeProvider, ServiceSpec, StepSpec},
    error::ForgeError,
};

/// Run all validation rules against a parsed configuration.
///
/// # Errors
///
/// Returns [`ForgeError::Validation`] if any rule fails.
pub fn validate(config: &ForgeConfig) -> Result<(), ForgeError> {
    check_api_version(config)?;
    check_kind(config)?;
    check_metadata_name(&config.metadata.name)?;
    check_network_name(&config.metadata.name, &config.spec)?;
    check_cluster_names(config)?;
    check_cluster_prefix(config)?;
    check_cluster_nodes(config)?;
    check_service_names(config)?;
    check_services(config)?;
    check_service_deps(config)?;
    check_service_auto_start_deps(config)?;
    check_service_dep_cycles(config)?;
    check_service_port_conflicts(config)?;
    check_stack_names(config)?;
    check_cluster_stack_refs(config)?;
    check_stack_steps(config)?;
    check_dns_zone(config)?;
    check_coredns_requires_cross_cluster(config)?;
    check_environment_network_requires_cross_cluster(config)?;
    check_cross_cluster_provider(config)?;
    check_no_templates(config)?;
    Ok(())
}

/// `apiVersion` must match the current schema.
fn check_api_version(config: &ForgeConfig) -> Result<(), ForgeError> {
    if config.api_version != API_VERSION {
        return Err(ForgeError::Validation(format!(
            "expected apiVersion {API_VERSION:?}, got {:?}",
            config.api_version,
        )));
    }
    Ok(())
}

/// `kind` must be `"Environment"`.
fn check_kind(config: &ForgeConfig) -> Result<(), ForgeError> {
    if config.kind != KIND {
        return Err(ForgeError::Validation(format!(
            "expected kind {KIND:?}, got {:?}",
            config.kind,
        )));
    }
    Ok(())
}

/// Validate that a name is a valid DNS label.
fn check_dns_label(name: &str, context: &str) -> Result<(), ForgeError> {
    if name.is_empty() {
        return Err(ForgeError::Validation(format!("{context}: name must not be empty")));
    }
    validate_dns_label_rules(name, context)
}

/// Check DNS label character and length rules.
fn validate_dns_label_rules(name: &str, context: &str) -> Result<(), ForgeError> {
    if name.len() > 63 {
        return Err(ForgeError::Validation(format!(
            "{context}: {name:?} exceeds 63 characters"
        )));
    }
    check_dns_label_chars(name, context)
}

/// Verify characters and leading/trailing constraints.
fn check_dns_label_chars(name: &str, context: &str) -> Result<(), ForgeError> {
    let valid = name
        .bytes()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'-');
    if !valid {
        return Err(ForgeError::Validation(format!(
            "{context}: {name:?} contains invalid characters \
             (allowed: lowercase alphanumeric and hyphens)"
        )));
    }
    check_dns_label_edges(name, context)
}

/// DNS labels must not start or end with a hyphen.
fn check_dns_label_edges(name: &str, context: &str) -> Result<(), ForgeError> {
    if name.starts_with('-') || name.ends_with('-') {
        return Err(ForgeError::Validation(format!(
            "{context}: {name:?} must not start or end with a hyphen"
        )));
    }
    Ok(())
}

/// `metadata.name` must be a DNS label.
fn check_metadata_name(name: &str) -> Result<(), ForgeError> {
    check_dns_label(name, "metadata.name")
}

/// Derived network name must be safe for Docker/Podman.
fn check_network_name(env_name: &str, spec: &crate::config::EnvironmentSpec) -> Result<(), ForgeError> {
    let wants = spec.network.as_ref().is_some_and(|n| n.cross_cluster);
    if !wants {
        return Ok(());
    }
    let derived = format!("{env_name}-net");
    check_docker_name(&derived, "derived network name")
}

/// Validate a Docker/Podman resource name.
fn check_docker_name(name: &str, context: &str) -> Result<(), ForgeError> {
    if name.is_empty() || name.len() > 128 {
        return Err(ForgeError::Validation(format!(
            "{context}: {name:?} must be 1\u{2013}128 characters"
        )));
    }
    let valid = name
        .bytes()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_' || ch == b'.');
    if !valid {
        return Err(ForgeError::Validation(format!(
            "{context}: {name:?} contains characters unsafe for Docker/Podman"
        )));
    }
    Ok(())
}

/// Cluster names must be unique and DNS-label-valid.
fn check_cluster_names(config: &ForgeConfig) -> Result<(), ForgeError> {
    let mut seen = BTreeSet::new();
    for cluster in &config.spec.clusters {
        check_dns_label(&cluster.name, "cluster")?;
        if !seen.insert(&cluster.name) {
            return Err(ForgeError::Validation(format!(
                "duplicate cluster name: {:?}",
                cluster.name,
            )));
        }
    }
    Ok(())
}

/// `runtime.clusterPrefix` must be a DNS label, and every derived
/// `{prefix}-{cluster}` KIND name must be safe for Docker/Podman.
///
/// The prefix flows verbatim into `kind create cluster --name` and the
/// temporary KIND config filename, so a bad value would otherwise
/// surface only as a raw tool error during `forge up`.
fn check_cluster_prefix(config: &ForgeConfig) -> Result<(), ForgeError> {
    let prefix = &config.spec.runtime.cluster_prefix;
    check_dns_label(prefix, "runtime.clusterPrefix")?;
    for cluster in &config.spec.clusters {
        let derived = crate::cluster::kind::kind_cluster_name(prefix, &cluster.name);
        check_docker_name(&derived, "derived KIND cluster name")?;
    }
    Ok(())
}

/// Maximum control-plane nodes per KIND cluster.
///
/// KIND is a development tool; the bound keeps an absurd count from
/// building a huge KIND config before kind itself could refuse it.
const MAX_CONTROL_PLANES: u32 = 9;

/// Maximum worker nodes per KIND cluster.
const MAX_WORKERS: u32 = 100;

/// Each cluster needs a bounded, non-zero control-plane count and a
/// bounded worker count.
fn check_cluster_nodes(config: &ForgeConfig) -> Result<(), ForgeError> {
    for cluster in &config.spec.clusters {
        let nodes = &cluster.nodes;
        if nodes.control_planes == 0 || nodes.control_planes > MAX_CONTROL_PLANES {
            return Err(ForgeError::Validation(format!(
                "cluster {:?}: controlPlanes must be 1..={MAX_CONTROL_PLANES}",
                cluster.name,
            )));
        }
        if nodes.workers > MAX_WORKERS {
            return Err(ForgeError::Validation(format!(
                "cluster {:?}: workers must not exceed {MAX_WORKERS}",
                cluster.name,
            )));
        }
    }
    Ok(())
}

/// Service names must be unique and DNS-label-valid.
fn check_service_names(config: &ForgeConfig) -> Result<(), ForgeError> {
    let mut seen = BTreeSet::new();
    for service in &config.spec.services {
        check_dns_label(&service.name, "service")?;
        if !seen.insert(&service.name) {
            return Err(ForgeError::Validation(format!(
                "duplicate service name: {:?}",
                service.name,
            )));
        }
    }
    Ok(())
}

/// Validate all fields on each service.
fn check_services(config: &ForgeConfig) -> Result<(), ForgeError> {
    for service in &config.spec.services {
        check_service_image(service)?;
        check_service_ports(service)?;
        check_service_volumes(service)?;
        check_service_env_keys(service)?;
        check_service_args_bounded(service)?;
        check_service_health_config(service)?;
    }
    Ok(())
}

/// Validate the container image field of a service.
fn check_service_image(svc: &ServiceSpec) -> Result<(), ForgeError> {
    let ctx = format!("service {:?}: image", svc.name);
    check_non_blank(&svc.image, &ctx)?;
    if svc.image.len() > 512 {
        return Err(ForgeError::Validation(format!("{ctx}: exceeds 512 characters")));
    }
    // Rejected here as well as terminated with `--` at the call site: no image
    // reference legally starts with '-', so this only ever refuses input that
    // was trying to become a runtime flag.
    if svc.image.starts_with('-') {
        return Err(ForgeError::Validation(format!("{ctx}: must not start with '-'")));
    }
    Ok(())
}

/// Validate all port mappings on a service.
fn check_service_ports(svc: &ServiceSpec) -> Result<(), ForgeError> {
    for port in &svc.ports {
        check_port_nonzero(port.host, &svc.name, "host")?;
        check_port_nonzero(port.container, &svc.name, "container")?;
        check_port_bind_address(port.bind_address.as_ref(), &svc.name)?;
        check_port_protocol_tcp(&port.protocol, &svc.name)?;
    }
    Ok(())
}

/// Reject port number zero.
fn check_port_nonzero(port: u16, svc_name: &str, field: &str) -> Result<(), ForgeError> {
    if port == 0 {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: {field} port must not be zero"
        )));
    }
    Ok(())
}

/// Validate an optional bind address as a valid IP.
fn check_port_bind_address(addr: Option<&String>, svc_name: &str) -> Result<(), ForgeError> {
    if let Some(addr) = addr.filter(|bind| bind.parse::<std::net::IpAddr>().is_err()) {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: bind address {addr:?} is not a valid IP"
        )));
    }
    Ok(())
}

/// F3 only allows TCP port protocol.
fn check_port_protocol_tcp(protocol: &str, svc_name: &str) -> Result<(), ForgeError> {
    if protocol != "tcp" {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: unsupported port protocol {protocol:?} \
             (expected tcp)"
        )));
    }
    Ok(())
}

/// Validate all volume mounts on a service.
fn check_service_volumes(svc: &ServiceSpec) -> Result<(), ForgeError> {
    for vol in &svc.volumes {
        let src_ctx = format!("service {:?}: volume source", svc.name);
        check_relative_path(&vol.source, &src_ctx)?;
        check_volume_target(&vol.target, &svc.name)?;
    }
    Ok(())
}

/// Volume target must be a non-empty absolute path.
fn check_volume_target(target: &str, svc_name: &str) -> Result<(), ForgeError> {
    if target.is_empty() {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: volume target must not be empty"
        )));
    }
    if !target.starts_with('/') {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: volume target must be an absolute path"
        )));
    }
    Ok(())
}

/// Validate all environment variable keys on a service.
fn check_service_env_keys(svc: &ServiceSpec) -> Result<(), ForgeError> {
    for key in svc.env.keys() {
        check_env_key(key, &svc.name)?;
    }
    Ok(())
}

/// Validate a single environment variable key.
fn check_env_key(key: &str, svc_name: &str) -> Result<(), ForgeError> {
    if key.is_empty() {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: environment key must not be empty"
        )));
    }
    if key.len() > 256 {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: environment key exceeds 256 characters"
        )));
    }
    if !is_shell_safe_ident(key) {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: environment key {key:?} is not a valid identifier"
        )));
    }
    Ok(())
}

/// Check whether a string matches shell-safe identifier `[A-Za-z_][A-Za-z0-9_]*`.
fn is_shell_safe_ident(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// Validate container argument count and individual length.
fn check_service_args_bounded(svc: &ServiceSpec) -> Result<(), ForgeError> {
    if svc.args.len() > 128 {
        return Err(ForgeError::Validation(format!(
            "service {:?}: more than 128 args",
            svc.name,
        )));
    }
    for arg in &svc.args {
        if arg.len() > 4096 {
            return Err(ForgeError::Validation(format!(
                "service {:?}: arg exceeds 4096 characters",
                svc.name,
            )));
        }
    }
    Ok(())
}

/// Validate health-check configuration if present.
fn check_service_health_config(svc: &ServiceSpec) -> Result<(), ForgeError> {
    let Some(hc) = &svc.health_check else {
        return Ok(());
    };
    check_health_check(hc, &svc.name)?;
    check_health_port_is_reachable(svc, hc)
}

/// Validate health-check fields.
fn check_health_check(hc: &HealthCheck, svc_name: &str) -> Result<(), ForgeError> {
    if hc.port == 0 {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: health check port must not be zero"
        )));
    }
    if hc.retries == 0 || hc.retries > 300 {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: health check retries must be 1..=300"
        )));
    }
    let interval_ctx = format!("service {svc_name:?}: health check interval");
    let timeout_ctx = format!("service {svc_name:?}: health check timeout");
    check_duration_string(&hc.interval, &interval_ctx)?;
    check_duration_string(&hc.timeout, &timeout_ctx)
}

/// Non-host-network services must publish the container health port.
fn check_health_port_is_reachable(svc: &ServiceSpec, hc: &HealthCheck) -> Result<(), ForgeError> {
    if matches!(svc.network, NetworkMode::Host) {
        return Ok(());
    }
    if svc
        .ports
        .iter()
        .any(|port| port.container == hc.port && port.protocol == "tcp")
    {
        return Ok(());
    }
    Err(ForgeError::Validation(format!(
        "service {:?}: health check port {} must match a published tcp container port \
         unless network is host",
        svc.name, hc.port
    )))
}

/// Upper bound for any configured duration, in seconds (24 hours).
///
/// Durations are added to [`std::time::Instant`], which panics on overflow, so
/// an unbounded value is a crash rather than a long wait. No development
/// environment operation legitimately waits longer than a day.
const MAX_DURATION_SECS: u64 = 86_400;

/// Validate a duration string (`"Ns"` or `"Nms"` where N is a positive integer).
fn check_duration_string(value: &str, context: &str) -> Result<(), ForgeError> {
    let (digits, max, unit) = if let Some(n) = value.strip_suffix("ms") {
        (n, MAX_DURATION_SECS.saturating_mul(1000), "ms")
    } else if let Some(n) = value.strip_suffix('s') {
        (n, MAX_DURATION_SECS, "s")
    } else {
        return Err(ForgeError::Validation(format!(
            "{context}: {value:?} must end in \"s\" or \"ms\""
        )));
    };
    let Ok(parsed) = digits.parse::<u64>() else {
        return Err(ForgeError::Validation(format!(
            "{context}: expected a positive integer"
        )));
    };
    if parsed == 0 {
        return Err(ForgeError::Validation(format!(
            "{context}: expected a positive integer"
        )));
    }
    if parsed > max {
        return Err(ForgeError::Validation(format!(
            "{context}: must not exceed {max}{unit}"
        )));
    }
    Ok(())
}

/// Validate that all service dependency references are valid.
fn check_service_deps(config: &ForgeConfig) -> Result<(), ForgeError> {
    let names: BTreeSet<&str> = config.spec.services.iter().map(|svc| svc.name.as_str()).collect();
    for svc in &config.spec.services {
        for dep in &svc.depends_on {
            check_single_dep(&svc.name, dep, &names)?;
        }
    }
    Ok(())
}

/// Validate a single dependency reference.
fn check_single_dep(svc_name: &str, dep: &str, known: &BTreeSet<&str>) -> Result<(), ForgeError> {
    if dep == svc_name {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: depends on itself"
        )));
    }
    if !known.contains(dep) {
        return Err(ForgeError::Validation(format!(
            "service {svc_name:?}: depends on unknown service {dep:?}"
        )));
    }
    Ok(())
}

/// Auto-started services cannot depend on services skipped by `forge up`.
fn check_service_auto_start_deps(config: &ForgeConfig) -> Result<(), ForgeError> {
    let auto_start: BTreeMap<&str, bool> = config
        .spec
        .services
        .iter()
        .map(|svc| (svc.name.as_str(), svc.auto_start))
        .collect();
    for svc in config.spec.services.iter().filter(|svc| svc.auto_start) {
        for dep in &svc.depends_on {
            if auto_start.get(dep.as_str()) == Some(&false) {
                return Err(ForgeError::Validation(format!(
                    "service {:?}: auto-started service depends on non-auto-start service {:?}",
                    svc.name, dep
                )));
            }
        }
    }
    Ok(())
}

/// Detect dependency cycles among services using topological sort.
fn check_service_dep_cycles(config: &ForgeConfig) -> Result<(), ForgeError> {
    let index = build_svc_name_index(&config.spec.services);
    let adj = build_svc_adjacency(&config.spec.services, &index);
    detect_dep_cycle(config.spec.services.len(), &adj)
}

/// Map each service name to its index in the services list.
fn build_svc_name_index(services: &[ServiceSpec]) -> BTreeMap<&str, usize> {
    services
        .iter()
        .enumerate()
        .map(|(idx, svc)| (svc.name.as_str(), idx))
        .collect()
}

/// Build adjacency list from dependency edges.
fn build_svc_adjacency(services: &[ServiceSpec], index: &BTreeMap<&str, usize>) -> Vec<Vec<usize>> {
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); services.len()];
    for (i, svc) in services.iter().enumerate() {
        for dep in &svc.depends_on {
            if let Some(entry) = index.get(dep.as_str()).and_then(|&j| adj.get_mut(j)) {
                entry.push(i);
            }
        }
    }
    adj
}

/// Run Kahn's algorithm to detect cycles.
fn detect_dep_cycle(count: usize, adj: &[Vec<usize>]) -> Result<(), ForgeError> {
    let mut in_deg = compute_in_degrees(count, adj);
    let visited = kahn_bfs(&mut in_deg, adj);
    if visited != count {
        return Err(ForgeError::Validation("service dependency cycle detected".to_owned()));
    }
    Ok(())
}

/// Compute in-degree for each node.
fn compute_in_degrees(count: usize, adj: &[Vec<usize>]) -> Vec<usize> {
    let mut in_deg: Vec<usize> = vec![0; count];
    for edges in adj {
        for &to in edges {
            if let Some(deg) = in_deg.get_mut(to) {
                *deg = deg.saturating_add(1);
            }
        }
    }
    in_deg
}

/// BFS from zero-degree nodes, returning the number of visited nodes.
fn kahn_bfs(in_deg: &mut [usize], adj: &[Vec<usize>]) -> usize {
    let mut queue: VecDeque<usize> = in_deg
        .iter()
        .enumerate()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(idx, _)| idx)
        .collect();
    let mut visited: usize = 0;
    while let Some(node) = queue.pop_front() {
        visited = visited.saturating_add(1);
        if let Some(edges) = adj.get(node) {
            for &to in edges {
                if let Some(deg) = in_deg.get_mut(to) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push_back(to);
                    }
                }
            }
        }
    }
    visited
}

/// A parsed bind address, classified for conflict detection.
#[derive(Debug, PartialEq, Eq)]
enum BindAddr {
    /// Publishes on all interfaces (unset, `0.0.0.0`, or `::`).
    Wildcard,
    /// Publishes on one specific address.
    Specific(std::net::IpAddr),
}

/// Classify an optional bind address for conflict detection.
///
/// Unparseable addresses are treated as wildcard; per-port validation
/// has already rejected them with a dedicated error by this point.
fn parse_bind_addr(addr: Option<&str>) -> BindAddr {
    match addr.and_then(|val| val.parse::<std::net::IpAddr>().ok()) {
        Some(ip) if !ip.is_unspecified() => BindAddr::Specific(ip),
        _ => BindAddr::Wildcard,
    }
}

/// True when a candidate binding overlaps any already-seen binding.
fn binds_conflict(seen: &[BindAddr], candidate: &BindAddr) -> bool {
    seen.iter()
        .any(|old| *old == BindAddr::Wildcard || *candidate == BindAddr::Wildcard || old == candidate)
}

/// Reject overlapping host-port bindings across services.
///
/// A wildcard bind (unset, `0.0.0.0`, or `::`) publishes on all
/// interfaces, so it conflicts with every other binding of the same
/// host port and protocol; two specific addresses conflict only when
/// they are the same IP.
fn check_service_port_conflicts(config: &ForgeConfig) -> Result<(), ForgeError> {
    let mut seen: BTreeMap<(u16, String), Vec<BindAddr>> = BTreeMap::new();
    for svc in &config.spec.services {
        for port in &svc.ports {
            let candidate = parse_bind_addr(port.bind_address.as_deref());
            let entry = seen.entry((port.host, port.protocol.clone())).or_default();
            if binds_conflict(entry, &candidate) {
                return Err(ForgeError::Validation(format!(
                    "duplicate host port binding: {}:{}/{}",
                    port.bind_address.as_deref().unwrap_or("0.0.0.0"),
                    port.host,
                    port.protocol,
                )));
            }
            entry.push(candidate);
        }
    }
    Ok(())
}

/// Stack names must be DNS-label-valid.
fn check_stack_names(config: &ForgeConfig) -> Result<(), ForgeError> {
    for name in config.spec.stacks.keys() {
        check_dns_label(name, "stack")?;
    }
    Ok(())
}

/// Validate every declared stack step.
fn check_stack_steps(config: &ForgeConfig) -> Result<(), ForgeError> {
    for (stack_name, stack) in &config.spec.stacks {
        for step in &stack.steps {
            check_step(stack_name, step)?;
        }
    }
    Ok(())
}

/// Validate a single stack step.
fn check_step(stack_name: &str, step: &StepSpec) -> Result<(), ForgeError> {
    match step {
        StepSpec::Url { url, sha256 } => check_url_step(stack_name, url, sha256),
        StepSpec::Manifest { path } | StepSpec::Kustomize { path } | StepSpec::TemplateManifest { path } => {
            check_step_path(stack_name, path)
        },
        StepSpec::Helm { .. } => check_helm_step(stack_name, step),
        StepSpec::Deployment {
            name, image, namespace, ..
        } => check_named_workload_step(stack_name, "deployment", name, image, namespace.as_deref()),
        StepSpec::Service { name, port, namespace } => {
            check_service_step(stack_name, name, *port, namespace.as_deref())
        },
        StepSpec::Wait {
            resource,
            condition,
            timeout,
            ..
        } => check_wait_step(stack_name, resource, condition, timeout),
        StepSpec::Exec { command, env } => check_exec_step(stack_name, command, env),
        StepSpec::ForEach { property, steps } => check_for_each_step(stack_name, property, steps),
        StepSpec::MetallbAutoPool { name } => check_named_resource_step(stack_name, "metallb pool", name, None),
        StepSpec::CoreDnsForward { zone, upstreams } => check_coredns_forward_step(stack_name, zone, upstreams),
        StepSpec::Capture { .. } => check_capture_step(stack_name, step),
        StepSpec::TemplateFile { source, target } => check_template_file_step(stack_name, source, target),
    }
}

/// Validate a template-file step.
fn check_template_file_step(stack_name: &str, source: &str, target: &str) -> Result<(), ForgeError> {
    check_relative_path(source, &format!("stack {stack_name:?}: template-file source"))?;
    check_relative_path(target, &format!("stack {stack_name:?}: template-file target"))
}

/// Validate a capture step.
fn check_capture_step(stack_name: &str, step: &StepSpec) -> Result<(), ForgeError> {
    let StepSpec::Capture {
        resource,
        namespace,
        jsonpath,
        key,
        timeout,
        interval,
    } = step
    else {
        return Ok(());
    };
    check_non_blank(resource, &format!("stack {stack_name:?}: capture resource"))?;
    check_not_option_like(resource, &format!("stack {stack_name:?}: capture resource"))?;
    check_non_blank(jsonpath, &format!("stack {stack_name:?}: capture jsonpath"))?;
    check_non_blank(key, &format!("stack {stack_name:?}: capture key"))?;
    check_duration_string(timeout, &format!("stack {stack_name:?}: capture timeout"))?;
    check_duration_string(interval, &format!("stack {stack_name:?}: capture interval"))?;
    if key.contains('.') {
        return Err(ForgeError::Validation(format!(
            "stack {stack_name:?}: capture key must not contain dots"
        )));
    }
    check_optional_namespace(stack_name, namespace.as_deref())
}

/// Validate a manifest or kustomize step path.
fn check_step_path(stack_name: &str, path: &str) -> Result<(), ForgeError> {
    check_relative_path(path, &format!("stack {stack_name:?}: path"))
}

/// Validate a `CoreDNS` forward step.
fn check_coredns_forward_step(stack_name: &str, zone: &str, upstreams: &[String]) -> Result<(), ForgeError> {
    let zone_ctx = format!("stack {stack_name:?}: coredns-forward zone");
    check_non_blank(zone, &zone_ctx)?;
    validate_dns_zone_rules(zone)
        .map_err(|_orig| ForgeError::Validation(format!("{zone_ctx}: {zone:?} is not a valid DNS zone")))?;
    if upstreams.is_empty() {
        return Err(ForgeError::Validation(format!(
            "stack {stack_name:?}: coredns-forward requires at least one upstream"
        )));
    }
    for upstream in upstreams {
        check_upstream_value(upstream, stack_name)?;
    }
    Ok(())
}

/// Validate a single `CoreDNS` upstream value.
///
/// Accepts: IPv4, IPv4:port, DNS hostname, DNS hostname:port.
fn check_upstream_value(value: &str, stack_name: &str) -> Result<(), ForgeError> {
    let ctx = format!("stack {stack_name:?}: coredns-forward upstream");
    check_non_blank(value, &ctx)?;
    if value.len() > 253 {
        return Err(ForgeError::Validation(format!("{ctx}: exceeds 253 characters")));
    }
    let (host, port) = split_upstream_host_port(value);
    if let Some(port_str) = port {
        check_upstream_port(port_str, &ctx)?;
    }
    check_upstream_host(host, &ctx)
}

/// Split an upstream into host and optional port on the last colon.
fn split_upstream_host_port(value: &str) -> (&str, Option<&str>) {
    if let Some(pos) = value.rfind(':') {
        let after = value.get(pos.saturating_add(1)..).unwrap_or("");
        if !after.is_empty() && after.bytes().all(|ch| ch.is_ascii_digit()) {
            return (value.get(..pos).unwrap_or(""), Some(after));
        }
    }
    (value, None)
}

/// Validate the port portion of an upstream.
fn check_upstream_port(port_str: &str, ctx: &str) -> Result<(), ForgeError> {
    let port: u32 = port_str
        .parse()
        .map_err(|_err| ForgeError::Validation(format!("{ctx}: invalid port {port_str:?}")))?;
    if port == 0 || port > 65535 {
        return Err(ForgeError::Validation(format!(
            "{ctx}: port {port} must be 1\u{2013}65535"
        )));
    }
    Ok(())
}

/// Validate the host portion as IPv4 or DNS hostname.
///
/// IPv4 parsing is attempted first; anything else is validated as a
/// DNS hostname, since RFC 1123 hostnames may begin with a digit
/// (e.g. `"0.pool.ntp.org"`).
fn check_upstream_host(host: &str, ctx: &str) -> Result<(), ForgeError> {
    if host.is_empty() {
        return Err(ForgeError::Validation(format!("{ctx}: empty host")));
    }
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return Ok(());
    }
    check_upstream_dns_name(host, ctx)
}

/// Validate a DNS hostname (dot-separated lowercase DNS labels).
fn check_upstream_dns_name(name: &str, ctx: &str) -> Result<(), ForgeError> {
    for label in name.split('.') {
        check_dns_label(label, ctx)?;
    }
    Ok(())
}

/// Validate a remote URL manifest step.
fn check_url_step(stack_name: &str, url: &str, sha256: &str) -> Result<(), ForgeError> {
    check_non_blank(url, &format!("stack {stack_name:?}: url"))?;
    if !url.starts_with("https://") {
        return Err(ForgeError::Validation(format!(
            "stack {stack_name:?}: remote manifest URLs must use https"
        )));
    }
    check_sha256(sha256, &format!("stack {stack_name:?}: sha256"))
}

/// Validate a SHA-256 hex digest.
///
/// Full-field template expressions are allowed; they must resolve to a
/// 64-character hex digest at apply time before content verification.
fn check_sha256(value: &str, context: &str) -> Result<(), ForgeError> {
    if is_full_field_template(value) {
        return Ok(());
    }
    if value.len() != 64 || !value.bytes().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(ForgeError::Validation(format!(
            "{context}: expected a 64-character SHA-256 hex digest"
        )));
    }
    Ok(())
}

/// True when `value` is a single `{{ ... }}` template expression.
fn is_full_field_template(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("{{") && trimmed.ends_with("}}") && trimmed.matches("{{").count() == 1
}

/// Validate a path that must stay relative to the config root.
fn check_relative_path(path: &str, context: &str) -> Result<(), ForgeError> {
    check_non_blank(path, context)?;
    if path.starts_with('/') || path.split('/').any(|part| part == "..") {
        return Err(ForgeError::Validation(format!(
            "{context}: path must be relative and must not escape the config root"
        )));
    }
    Ok(())
}

/// Reject a value that a tool would parse as an option rather than a positional.
///
/// `helm` and `kubectl` accept flags interspersed with positional arguments, so
/// a configured chart or resource beginning with `-` is consumed as a flag —
/// `--post-renderer=...` on a helm chart runs an arbitrary program. None of
/// these fields legitimately starts with `-`, so refusing one only rejects
/// input that was trying to become a flag.
fn check_not_option_like(value: &str, context: &str) -> Result<(), ForgeError> {
    if value.starts_with('-') {
        return Err(ForgeError::Validation(format!("{context}: must not start with '-'")));
    }
    Ok(())
}

/// Validate a Helm step.
fn check_helm_step(stack_name: &str, step: &StepSpec) -> Result<(), ForgeError> {
    let StepSpec::Helm {
        release,
        chart,
        version,
        namespace,
        ..
    } = step
    else {
        return Ok(());
    };
    check_dns_label(release, &format!("stack {stack_name:?}: helm release"))?;
    check_non_blank(chart, &format!("stack {stack_name:?}: helm chart"))?;
    check_not_option_like(chart, &format!("stack {stack_name:?}: helm chart"))?;
    check_non_blank(version, &format!("stack {stack_name:?}: helm version"))?;
    check_not_option_like(version, &format!("stack {stack_name:?}: helm version"))?;
    check_optional_namespace(stack_name, namespace.as_deref())
}

/// Validate a resource step with a Kubernetes-style name.
fn check_named_resource_step(
    stack_name: &str,
    kind: &str,
    name: &str,
    namespace: Option<&str>,
) -> Result<(), ForgeError> {
    check_dns_label(name, &format!("stack {stack_name:?}: {kind} name"))?;
    check_optional_namespace(stack_name, namespace)
}

/// Validate a workload step that includes an image.
fn check_named_workload_step(
    stack_name: &str,
    kind: &str,
    name: &str,
    image: &str,
    namespace: Option<&str>,
) -> Result<(), ForgeError> {
    check_named_resource_step(stack_name, kind, name, namespace)?;
    check_non_blank(image, &format!("stack {stack_name:?}: {kind} image"))
}

/// Validate a generated Service step.
fn check_service_step(stack_name: &str, name: &str, port: u16, namespace: Option<&str>) -> Result<(), ForgeError> {
    check_named_resource_step(stack_name, "service", name, namespace)?;
    if port == 0 {
        return Err(ForgeError::Validation(format!(
            "stack {stack_name:?}: service port must not be zero"
        )));
    }
    Ok(())
}

/// Validate an optional namespace.
fn check_optional_namespace(stack_name: &str, namespace: Option<&str>) -> Result<(), ForgeError> {
    if let Some(ns) = namespace {
        check_dns_label(ns, &format!("stack {stack_name:?}: namespace"))?;
    }
    Ok(())
}

/// Validate a wait step.
fn check_wait_step(stack_name: &str, resource: &str, condition: &str, timeout: &str) -> Result<(), ForgeError> {
    check_non_blank(resource, &format!("stack {stack_name:?}: wait resource"))?;
    check_not_option_like(resource, &format!("stack {stack_name:?}: wait resource"))?;
    check_non_blank(condition, &format!("stack {stack_name:?}: wait condition"))?;
    check_duration_string(timeout, &format!("stack {stack_name:?}: wait timeout"))
}

/// Validate an exec step.
fn check_exec_step(stack_name: &str, command: &[String], env: &BTreeMap<String, String>) -> Result<(), ForgeError> {
    if command.is_empty() {
        return Err(ForgeError::Validation(format!(
            "stack {stack_name:?}: exec command must not be empty"
        )));
    }
    for arg in command {
        check_non_blank(arg, &format!("stack {stack_name:?}: exec command argument"))?;
    }
    for (key, value) in env {
        check_exec_env_key(stack_name, key)?;
        check_non_blank(value, &format!("stack {stack_name:?}: exec env value for {key:?}"))?;
    }
    Ok(())
}

/// Validate an exec environment variable name.
fn check_exec_env_key(stack_name: &str, key: &str) -> Result<(), ForgeError> {
    check_non_blank(key, &format!("stack {stack_name:?}: exec env key"))?;
    if !is_shell_safe_ident(key) {
        return Err(ForgeError::Validation(format!(
            "stack {stack_name:?}: exec env key {key:?} is not a valid identifier"
        )));
    }
    Ok(())
}

/// Validate a for-each step.
fn check_for_each_step(stack_name: &str, property: &str, steps: &[StepSpec]) -> Result<(), ForgeError> {
    check_non_blank(property, &format!("stack {stack_name:?}: for-each property"))?;
    if steps.is_empty() {
        return Err(ForgeError::Validation(format!(
            "stack {stack_name:?}: for-each steps must not be empty"
        )));
    }
    for step in steps {
        check_step(stack_name, step)?;
    }
    Ok(())
}

/// Check a required text field.
fn check_non_blank(value: &str, context: &str) -> Result<(), ForgeError> {
    if value.trim().is_empty() {
        return Err(ForgeError::Validation(format!("{context}: must not be blank")));
    }
    Ok(())
}

/// Every stack referenced by a cluster must exist in `spec.stacks`.
fn check_cluster_stack_refs(config: &ForgeConfig) -> Result<(), ForgeError> {
    for cluster in &config.spec.clusters {
        for stack_ref in &cluster.stacks {
            if !config.spec.stacks.contains_key(stack_ref) {
                return Err(ForgeError::Validation(format!(
                    "cluster {:?} references unknown stack {:?}",
                    cluster.name, stack_ref,
                )));
            }
        }
    }
    Ok(())
}

/// Reject template syntax (`{{ ... }}`) outside stack steps.
///
/// Stack steps may contain template expressions resolved at apply
/// time.  All other config fields must be template-free.
fn check_no_templates(config: &ForgeConfig) -> Result<(), ForgeError> {
    let mut sanitized = config.clone();
    sanitized.spec.stacks.clear();
    let yaml = serde_yaml::to_string(&sanitized).map_err(|err| ForgeError::Validation(err.to_string()))?;
    if yaml.contains("{{") && yaml.contains("}}") {
        return Err(ForgeError::Validation(
            "template syntax ({{ ... }}) is not supported outside \
             stack steps"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Validate `spec.network.dnsZone` when set.
fn check_dns_zone(config: &ForgeConfig) -> Result<(), ForgeError> {
    let Some(zone) = config.spec.network.as_ref().and_then(|n| n.dns_zone.as_deref()) else {
        return Ok(());
    };
    validate_dns_zone_rules(zone)
}

/// DNS zone format rules: lowercase alphanumeric/hyphens/dots, at least one dot.
fn validate_dns_zone_rules(zone: &str) -> Result<(), ForgeError> {
    if zone.is_empty() || zone.len() > 253 {
        return Err(ForgeError::Validation("dnsZone must be 1-253 characters".to_owned()));
    }
    if !zone
        .bytes()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'-' || ch == b'.')
    {
        return Err(ForgeError::Validation(
            "dnsZone must contain only lowercase alphanumeric, hyphens, and dots".to_owned(),
        ));
    }
    if zone.starts_with('.') || zone.starts_with('-') || zone.ends_with('.') || zone.ends_with('-') {
        return Err(ForgeError::Validation(
            "dnsZone must not start or end with a dot or hyphen".to_owned(),
        ));
    }
    if !zone.contains('.') {
        return Err(ForgeError::Validation(
            "dnsZone must contain at least one dot".to_owned(),
        ));
    }
    Ok(())
}

/// Any `CoreDnsForward` step requires `spec.network.crossCluster: true`.
fn check_coredns_requires_cross_cluster(config: &ForgeConfig) -> Result<(), ForgeError> {
    let has_cross = config.spec.network.as_ref().is_some_and(|n| n.cross_cluster);
    if has_cross {
        return Ok(());
    }
    for (name, stack) in &config.spec.stacks {
        for step in &stack.steps {
            if matches!(step, StepSpec::CoreDnsForward { .. }) {
                return Err(ForgeError::Validation(format!(
                    "stack {name:?}: core-dns-forward requires spec.network.crossCluster: true"
                )));
            }
        }
    }
    Ok(())
}

/// Any `network: environment` service requires `spec.network.crossCluster: true`.
///
/// Such a service is attached to the shared `{env}-net` container
/// network, which `forge up` only creates when cross-cluster networking
/// is enabled; without this rule the mismatch surfaces as a raw
/// "network not found" docker error after clusters were already created.
fn check_environment_network_requires_cross_cluster(config: &ForgeConfig) -> Result<(), ForgeError> {
    let has_cross = config.spec.network.as_ref().is_some_and(|n| n.cross_cluster);
    if has_cross {
        return Ok(());
    }
    for svc in &config.spec.services {
        if svc.network == NetworkMode::Environment {
            return Err(ForgeError::Validation(format!(
                "service {:?}: network: environment requires spec.network.crossCluster: true",
                svc.name,
            )));
        }
    }
    Ok(())
}

/// Cross-cluster networking requires Docker; reject explicit Podman.
fn check_cross_cluster_provider(config: &ForgeConfig) -> Result<(), ForgeError> {
    let wants_cross = config.spec.network.as_ref().is_some_and(|n| n.cross_cluster);
    if !wants_cross {
        return Ok(());
    }
    if config.spec.runtime.provider == RuntimeProvider::Podman {
        return Err(ForgeError::Validation(
            "cross-cluster networking requires Docker; Podman is not supported in this phase".to_owned(),
        ));
    }
    Ok(())
}

// -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ClusterSpec, EnvironmentSpec, HealthCheckType, Metadata, NetworkConfig, NodeConfig, PortMapping, RestartPolicy,
        RuntimeConfig, StackSpec, VolumeMount,
    };

    /// Build a minimal valid config for test modification.
    fn base_config() -> ForgeConfig {
        ForgeConfig {
            api_version: API_VERSION.to_owned(),
            kind: KIND.to_owned(),
            metadata: Metadata {
                name: "test".to_owned(),
            },
            spec: EnvironmentSpec {
                runtime: RuntimeConfig::default(),
                network: None,
                clusters: Vec::new(),
                services: Vec::new(),
                certificates: None,
                stacks: BTreeMap::new(),
            },
        }
    }

    /// Build a minimal valid service for testing.
    fn test_service(name: &str) -> ServiceSpec {
        ServiceSpec {
            name: name.to_owned(),
            image: "example/svc:v1".to_owned(),
            auto_start: true,
            network: NetworkMode::None,
            depends_on: Vec::new(),
            ports: Vec::new(),
            volumes: Vec::new(),
            env: BTreeMap::new(),
            args: Vec::new(),
            restart: RestartPolicy::No,
            inherit_host_group: false,
            health_check: None,
        }
    }

    /// Build a service with one custom port mapping.
    fn test_service_with_port(port: PortMapping) -> ServiceSpec {
        let mut svc = test_service("svc");
        svc.ports = vec![port];
        svc
    }

    #[test]
    fn valid_minimal_config_passes() {
        let config = base_config();
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn wrong_api_version_rejected() {
        let mut config = base_config();
        config.api_version = "v2".to_owned();
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("apiVersion"), "expected apiVersion error, got: {msg}");
    }

    #[test]
    fn wrong_kind_rejected() {
        let mut config = base_config();
        config.kind = "Cluster".to_owned();
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("kind"), "expected kind error, got: {msg}");
    }

    #[test]
    fn empty_name_rejected() {
        let mut config = base_config();
        config.metadata.name = String::new();
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("empty"), "expected empty name error, got: {msg}");
    }

    #[test]
    fn non_dns_name_rejected() {
        let mut config = base_config();
        config.metadata.name = "Not_Valid".to_owned();
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("invalid characters"), "expected DNS error, got: {msg}");
    }

    #[test]
    fn leading_hyphen_name_rejected() {
        let mut config = base_config();
        config.metadata.name = "-bad".to_owned();
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("hyphen"), "expected hyphen error, got: {msg}");
    }

    #[test]
    fn empty_cluster_prefix_rejected() {
        let mut config = base_config();
        config.spec.runtime.cluster_prefix = String::new();
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("clusterPrefix"),
            "expected clusterPrefix error, got: {msg}"
        );
    }

    #[test]
    fn invalid_cluster_prefix_rejected() {
        for bad in ["Forge", "with space", "with/slash", "-lead", "x".repeat(64).as_str()] {
            let mut config = base_config();
            config.spec.runtime.cluster_prefix = bad.to_owned();
            assert!(validate(&config).is_err(), "should reject clusterPrefix {bad:?}");
        }
    }

    #[test]
    fn custom_cluster_prefix_passes() {
        let mut config = base_config();
        config.spec.runtime.cluster_prefix = "dev-env2".to_owned();
        config.spec.clusters = vec![ClusterSpec {
            name: "hub".to_owned(),
            nodes: NodeConfig::default(),
            stacks: Vec::new(),
            properties: BTreeMap::new(),
        }];
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn duplicate_cluster_names_rejected() {
        let mut config = base_config();
        let cluster = ClusterSpec {
            name: "dupe".to_owned(),
            nodes: NodeConfig::default(),
            stacks: Vec::new(),
            properties: BTreeMap::new(),
        };
        config.spec.clusters = vec![cluster.clone(), cluster];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate cluster"),
            "expected duplicate error, got: {msg}"
        );
    }

    #[test]
    fn cluster_referencing_missing_stack_rejected() {
        let mut config = base_config();
        config.spec.clusters = vec![ClusterSpec {
            name: "c1".to_owned(),
            nodes: NodeConfig::default(),
            stacks: vec!["nonexistent".to_owned()],
            properties: BTreeMap::new(),
        }];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("unknown stack"),
            "expected missing stack error, got: {msg}"
        );
    }

    #[test]
    fn template_looking_values_rejected() {
        let mut config = base_config();
        config.spec.clusters = vec![ClusterSpec {
            name: "c1".to_owned(),
            nodes: NodeConfig::default(),
            stacks: Vec::new(),
            properties: BTreeMap::from([(
                "model".to_owned(),
                serde_json::Value::String("{{ .Property.model }}".to_owned()),
            )]),
        }];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("template syntax"), "expected template error, got: {msg}");
    }

    #[test]
    fn templates_in_stack_steps_pass_validation() {
        let mut config = base_config();
        config.spec.stacks = BTreeMap::from([(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Manifest {
                    path: "{{ cluster.name }}/manifests".to_owned(),
                }],
            },
        )]);
        config.spec.clusters = vec![ClusterSpec {
            name: "hub".to_owned(),
            nodes: NodeConfig::default(),
            stacks: vec!["base".to_owned()],
            properties: BTreeMap::new(),
        }];
        assert!(validate(&config).is_ok(), "templates in stack steps should be allowed");
    }

    #[test]
    fn templates_outside_stacks_still_rejected() {
        let mut config = base_config();
        config.spec.services = vec![{
            let mut svc = test_service("bad");
            svc.image = "{{ cluster.name }}/img:v1".to_owned();
            svc
        }];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("template syntax"),
            "templates in service image should be rejected: {msg}"
        );
    }

    #[test]
    fn zero_control_planes_rejected() {
        let mut config = base_config();
        config.spec.clusters = vec![ClusterSpec {
            name: "c1".to_owned(),
            nodes: NodeConfig {
                control_planes: 0,
                workers: 1,
            },
            stacks: Vec::new(),
            properties: BTreeMap::new(),
        }];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("controlPlanes"),
            "expected control-plane count error, got: {msg}"
        );
    }

    #[test]
    fn excessive_control_planes_rejected() {
        let mut config = base_config();
        config.spec.clusters = vec![ClusterSpec {
            name: "big".to_owned(),
            nodes: NodeConfig {
                control_planes: 10,
                workers: 0,
            },
            stacks: Vec::new(),
            properties: BTreeMap::new(),
        }];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("big") && msg.contains("controlPlanes"),
            "expected named control-plane bound error, got: {msg}"
        );
    }

    #[test]
    fn excessive_workers_rejected() {
        let mut config = base_config();
        config.spec.clusters = vec![ClusterSpec {
            name: "big".to_owned(),
            nodes: NodeConfig {
                control_planes: 1,
                workers: u32::MAX,
            },
            stacks: Vec::new(),
            properties: BTreeMap::new(),
        }];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("big") && msg.contains("workers"),
            "expected named worker bound error, got: {msg}"
        );
    }

    #[test]
    fn maximum_node_counts_pass() {
        let mut config = base_config();
        config.spec.clusters = vec![ClusterSpec {
            name: "big".to_owned(),
            nodes: NodeConfig {
                control_planes: 9,
                workers: 100,
            },
            stacks: Vec::new(),
            properties: BTreeMap::new(),
        }];
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn invalid_service_protocol_rejected() {
        let mut config = base_config();
        config.spec.services = vec![test_service_with_port(PortMapping {
            bind_address: None,
            host: 8080,
            container: 8080,
            protocol: "sctp".to_owned(),
        })];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("protocol"), "expected protocol error, got: {msg}");
    }

    #[test]
    fn unpinned_remote_url_rejected_by_schema() {
        let yaml = "\
apiVersion: forge.praxis.dev/v1alpha1
kind: Environment
metadata:
  name: test
spec:
  runtime:
    provider: auto
  stacks:
    base:
      steps:
        - type: url
          url: https://example.invalid/install.yaml
";
        let result: Result<ForgeConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "url steps must require sha256");
    }

    #[test]
    fn non_https_remote_url_rejected() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Url {
                    url: "http://example.invalid/install.yaml".to_owned(),
                    sha256: "a".repeat(64),
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("https"), "expected https error, got: {msg}");
    }

    #[test]
    fn templated_sha256_accepted() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Url {
                    url: "https://example.invalid/install.yaml".to_owned(),
                    sha256: "{{ cluster.properties.artifactSha256 }}".to_owned(),
                }],
            },
        );
        assert!(
            validate(&config).is_ok(),
            "full-field sha256 templates must be allowed at validate time"
        );
    }

    #[test]
    fn invalid_sha256_rejected() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Url {
                    url: "https://example.invalid/install.yaml".to_owned(),
                    sha256: "not-a-digest".to_owned(),
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("SHA-256"), "expected digest error, got: {msg}");
    }

    #[test]
    fn invalid_exec_env_key_rejected() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Exec {
                    command: vec!["true".to_owned()],
                    env: BTreeMap::from([("BAD-KEY".to_owned(), "x".to_owned())]),
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("env key"), "expected env key error, got: {msg}");
    }

    #[test]
    fn empty_exec_command_rejected() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Exec {
                    command: Vec::new(),
                    env: BTreeMap::new(),
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("exec command"), "expected exec error, got: {msg}");
    }

    #[test]
    fn stack_service_zero_port_rejected() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Service {
                    name: "web".to_owned(),
                    port: 0,
                    namespace: None,
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("port"), "expected service port error, got: {msg}");
    }

    #[test]
    fn stack_wait_timeout_format_rejected() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Wait {
                    resource: "deployment/web".to_owned(),
                    condition: "available".to_owned(),
                    timeout: "soon".to_owned(),
                    namespace: None,
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("timeout"), "expected wait timeout error, got: {msg}");
    }

    #[test]
    fn stack_helm_chart_starting_with_dash_rejected() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Helm {
                    release: "web".to_owned(),
                    // helm parses flags interspersed with positionals, so this
                    // would run an arbitrary program at render time.
                    chart: "--post-renderer=/tmp/evil.sh".to_owned(),
                    version: "1.0.0".to_owned(),
                    namespace: None,
                    values: BTreeMap::new(),
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("must not start with '-'"),
            "expected helm chart dash rejection, got: {msg}"
        );
    }

    #[test]
    fn stack_wait_resource_starting_with_dash_rejected() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Wait {
                    resource: "--kubeconfig=/tmp/evil".to_owned(),
                    condition: "available".to_owned(),
                    timeout: "30s".to_owned(),
                    namespace: None,
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("must not start with '-'"),
            "expected wait resource dash rejection, got: {msg}"
        );
    }

    #[test]
    fn network_cross_cluster_passes_validation() {
        let mut config = base_config();
        config.spec.network = Some(NetworkConfig {
            cross_cluster: true,
            dns_zone: None,
        });
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn network_config_without_cross_cluster_passes() {
        let mut config = base_config();
        config.spec.network = Some(NetworkConfig {
            cross_cluster: false,
            dns_zone: None,
        });
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn service_with_full_spec_passes() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.ports = vec![PortMapping {
            bind_address: Some("127.0.0.1".to_owned()),
            host: 8080,
            container: 80,
            protocol: "tcp".to_owned(),
        }];
        svc.volumes = vec![VolumeMount {
            source: "data".to_owned(),
            target: "/data".to_owned(),
            read_only: false,
        }];
        svc.env = BTreeMap::from([("HOME".to_owned(), "/root".to_owned())]);
        svc.args = vec!["--port".to_owned(), "80".to_owned()];
        svc.health_check = Some(HealthCheck {
            check_type: HealthCheckType::Tcp,
            port: 80,
            interval: "2s".to_owned(),
            timeout: "1s".to_owned(),
            retries: 3,
        });
        config.spec.services = vec![svc];
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn service_self_dependency_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.depends_on = vec!["web".to_owned()];
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("depends on itself"), "expected self-dep error, got: {msg}");
    }

    #[test]
    fn service_unknown_dependency_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.depends_on = vec!["nonexistent".to_owned()];
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("unknown service"),
            "expected unknown dep error, got: {msg}"
        );
    }

    #[test]
    fn service_dependency_cycle_rejected() {
        let mut config = base_config();
        let mut svc_a = test_service("a");
        svc_a.depends_on = vec!["b".to_owned()];
        let mut svc_b = test_service("b");
        svc_b.depends_on = vec!["a".to_owned()];
        config.spec.services = vec![svc_a, svc_b];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("cycle"), "expected cycle error, got: {msg}");
    }

    #[test]
    fn auto_start_service_cannot_depend_on_non_auto_start_service() {
        let mut config = base_config();
        let mut sync = test_service("sync");
        sync.auto_start = false;
        let mut edge = test_service("edge");
        edge.depends_on = vec!["sync".to_owned()];
        config.spec.services = vec![sync, edge];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("auto-started service depends on non-auto-start service"),
            "expected auto-start dependency error, got: {msg}"
        );
    }

    #[test]
    fn non_auto_start_service_may_depend_on_non_auto_start_service() {
        let mut config = base_config();
        let mut sync = test_service("sync");
        sync.auto_start = false;
        let mut edge = test_service("edge");
        edge.auto_start = false;
        edge.depends_on = vec!["sync".to_owned()];
        config.spec.services = vec![sync, edge];
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn service_bind_address_invalid_rejected() {
        let mut config = base_config();
        config.spec.services = vec![test_service_with_port(PortMapping {
            bind_address: Some("not-an-ip".to_owned()),
            host: 8080,
            container: 8080,
            protocol: "tcp".to_owned(),
        })];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("bind address"), "expected bind address error, got: {msg}");
    }

    #[test]
    fn service_bind_address_valid_passes() {
        let mut config = base_config();
        config.spec.services = vec![test_service_with_port(PortMapping {
            bind_address: Some("127.0.0.1".to_owned()),
            host: 8080,
            container: 8080,
            protocol: "tcp".to_owned(),
        })];
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn service_port_zero_rejected() {
        let mut config = base_config();
        config.spec.services = vec![test_service_with_port(PortMapping {
            bind_address: None,
            host: 0,
            container: 8080,
            protocol: "tcp".to_owned(),
        })];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("port must not be zero"),
            "expected port zero error, got: {msg}"
        );
    }

    #[test]
    fn service_image_starting_with_dash_rejected() {
        let mut config = base_config();
        let mut svc = test_service_with_port(PortMapping {
            bind_address: None,
            host: 8080,
            container: 8080,
            protocol: "tcp".to_owned(),
        });
        svc.image = "--volume=/:/host".to_owned();
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("must not start with '-'"),
            "expected leading-dash image rejection, got: {msg}"
        );
    }

    #[test]
    fn service_duplicate_host_port_rejected() {
        let mut config = base_config();
        let svc_a = test_service_with_port(PortMapping {
            bind_address: None,
            host: 8080,
            container: 80,
            protocol: "tcp".to_owned(),
        });
        let mut svc_b = test_service("other");
        svc_b.ports = vec![PortMapping {
            bind_address: None,
            host: 8080,
            container: 90,
            protocol: "tcp".to_owned(),
        }];
        config.spec.services = vec![svc_a, svc_b];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("duplicate"), "expected duplicate port error, got: {msg}");
    }

    /// Build two single-port services with the given bind addresses on port 8080.
    fn two_services_with_binds(bind_a: Option<&str>, bind_b: Option<&str>) -> ForgeConfig {
        let mut config = base_config();
        let svc_a = test_service_with_port(PortMapping {
            bind_address: bind_a.map(str::to_owned),
            host: 8080,
            container: 80,
            protocol: "tcp".to_owned(),
        });
        let mut svc_b = test_service("other");
        svc_b.ports = vec![PortMapping {
            bind_address: bind_b.map(str::to_owned),
            host: 8080,
            container: 90,
            protocol: "tcp".to_owned(),
        }];
        config.spec.services = vec![svc_a, svc_b];
        config
    }

    #[test]
    fn unset_bind_conflicts_with_explicit_wildcard() {
        let config = two_services_with_binds(None, Some("0.0.0.0"));
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("duplicate"), "expected duplicate port error, got: {msg}");
    }

    #[test]
    fn unset_bind_conflicts_with_specific_address() {
        let config = two_services_with_binds(None, Some("127.0.0.1"));
        assert!(
            validate(&config).is_err(),
            "unset bind publishes on all interfaces and must conflict with 127.0.0.1"
        );
    }

    #[test]
    fn same_specific_bind_conflicts() {
        let config = two_services_with_binds(Some("127.0.0.1"), Some("127.0.0.1"));
        assert!(validate(&config).is_err(), "same specific bind address must conflict");
    }

    #[test]
    fn distinct_specific_binds_pass() {
        let config = two_services_with_binds(Some("127.0.0.1"), Some("192.168.0.10"));
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn service_volume_source_escape_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.volumes = vec![VolumeMount {
            source: "../etc/passwd".to_owned(),
            target: "/data".to_owned(),
            read_only: false,
        }];
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("escape"), "expected path escape error, got: {msg}");
    }

    #[test]
    fn service_volume_target_relative_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.volumes = vec![VolumeMount {
            source: "data".to_owned(),
            target: "relative/path".to_owned(),
            read_only: false,
        }];
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("absolute"), "expected absolute path error, got: {msg}");
    }

    #[test]
    fn service_with_dotforge_runtime_volume_valid() {
        let mut config = base_config();
        let mut svc = test_service("edge");
        svc.volumes = vec![VolumeMount {
            source: ".forge/runtime/edge-us-east".to_owned(),
            target: "/etc/grid".to_owned(),
            read_only: true,
        }];
        config.spec.services = vec![svc];
        validate(&config).unwrap_or_else(|_| std::process::abort());
    }

    #[test]
    fn service_with_localhost_bind_address_valid() {
        let mut config = base_config();
        let mut svc = test_service("edge");
        svc.ports = vec![PortMapping {
            bind_address: Some("127.0.0.1".to_owned()),
            host: 8080,
            container: 8080,
            protocol: "tcp".to_owned(),
        }];
        config.spec.services = vec![svc];
        validate(&config).unwrap_or_else(|_| std::process::abort());
    }

    #[test]
    fn service_env_key_empty_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.env = BTreeMap::from([(String::new(), "val".to_owned())]);
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("environment key"), "expected env key error, got: {msg}");
    }

    #[test]
    fn service_env_key_invalid_chars_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.env = BTreeMap::from([("MY-KEY".to_owned(), "val".to_owned())]);
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("not a valid identifier"),
            "expected invalid key error, got: {msg}"
        );
    }

    #[test]
    fn service_health_retries_zero_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.health_check = Some(HealthCheck {
            check_type: HealthCheckType::Tcp,
            port: 80,
            interval: "2s".to_owned(),
            timeout: "1s".to_owned(),
            retries: 0,
        });
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("retries"), "expected retries error, got: {msg}");
    }

    #[test]
    fn service_health_bad_interval_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.health_check = Some(HealthCheck {
            check_type: HealthCheckType::Tcp,
            port: 80,
            interval: "abc".to_owned(),
            timeout: "1s".to_owned(),
            retries: 3,
        });
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("interval"), "expected interval error, got: {msg}");
    }

    #[test]
    fn service_health_unpublished_port_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.ports = vec![PortMapping {
            bind_address: Some("127.0.0.1".to_owned()),
            host: 8080,
            container: 80,
            protocol: "tcp".to_owned(),
        }];
        svc.health_check = Some(HealthCheck {
            check_type: HealthCheckType::Tcp,
            port: 81,
            interval: "2s".to_owned(),
            timeout: "1s".to_owned(),
            retries: 3,
        });
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("published tcp container port"),
            "expected unpublished health port error, got: {msg}"
        );
    }

    #[test]
    fn service_image_too_long_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.image = "x".repeat(513);
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("512"), "expected image length error, got: {msg}");
    }

    #[test]
    fn service_args_too_many_rejected() {
        let mut config = base_config();
        let mut svc = test_service("web");
        svc.args = (0..129).map(|i| format!("arg{i}")).collect();
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("128"), "expected args count error, got: {msg}");
    }

    #[test]
    fn dns_zone_valid_passes() {
        let mut config = base_config();
        config.spec.network = Some(NetworkConfig {
            cross_cluster: true,
            dns_zone: Some("forge.test".to_owned()),
        });
        assert!(validate(&config).is_ok(), "forge.test should be a valid dns zone");
    }

    #[test]
    fn dns_zone_invalid_rejected() {
        let mut config = base_config();
        config.spec.network = Some(NetworkConfig {
            cross_cluster: true,
            dns_zone: Some("UPPER.case".to_owned()),
        });
        assert!(validate(&config).is_err(), "uppercase should be rejected");

        config.spec.network = Some(NetworkConfig {
            cross_cluster: true,
            dns_zone: Some(".leading-dot".to_owned()),
        });
        assert!(validate(&config).is_err(), "leading dot should be rejected");

        config.spec.network = Some(NetworkConfig {
            cross_cluster: true,
            dns_zone: Some("nodot".to_owned()),
        });
        assert!(validate(&config).is_err(), "no dot should be rejected");
    }

    #[test]
    fn coredns_forward_requires_cross_cluster() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "net".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::CoreDnsForward {
                    zone: "forge.test".to_owned(),
                    upstreams: vec!["10.0.0.1".to_owned()],
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("crossCluster"), "should mention crossCluster: {msg}");
    }

    #[test]
    fn environment_network_service_requires_cross_cluster() {
        let mut config = base_config();
        let mut svc = test_service("edge");
        svc.network = NetworkMode::Environment;
        config.spec.services = vec![svc];
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(
            msg.contains("edge") && msg.contains("crossCluster"),
            "expected crossCluster requirement error, got: {msg}"
        );
    }

    #[test]
    fn environment_network_service_rejected_when_cross_cluster_false() {
        let mut config = base_config();
        config.spec.network = Some(NetworkConfig {
            cross_cluster: false,
            dns_zone: None,
        });
        let mut svc = test_service("edge");
        svc.network = NetworkMode::Environment;
        config.spec.services = vec![svc];
        assert!(
            validate(&config).is_err(),
            "crossCluster: false should not satisfy a network: environment service"
        );
    }

    #[test]
    fn environment_network_service_passes_with_cross_cluster() {
        let mut config = base_config();
        config.spec.network = Some(NetworkConfig {
            cross_cluster: true,
            dns_zone: None,
        });
        let mut svc = test_service("edge");
        svc.network = NetworkMode::Environment;
        config.spec.services = vec![svc];
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn coredns_forward_zone_validated_as_dns() {
        let mut config = base_config();
        config.spec.network = Some(NetworkConfig {
            cross_cluster: true,
            dns_zone: None,
        });
        config.spec.stacks.insert(
            "net".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::CoreDnsForward {
                    zone: "UPPER.BAD".to_owned(),
                    upstreams: vec!["10.0.0.1".to_owned()],
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("valid DNS zone"), "should reject invalid zone: {msg}");
    }

    #[test]
    fn coredns_forward_upstream_injection_rejected() {
        let cases = [
            "10.0.0.1; rm -rf /",
            ":::::",
            "host:",
            "host:0",
            "host:99999",
            "10.0.0.1:abc",
            "bad..name",
            "-bad.name",
            "bad.name-",
        ];
        for bad in cases {
            let mut config = base_config();
            config.spec.network = Some(NetworkConfig {
                cross_cluster: true,
                dns_zone: None,
            });
            config.spec.stacks.insert(
                "net".to_owned(),
                StackSpec {
                    description: None,
                    steps: vec![StepSpec::CoreDnsForward {
                        zone: "forge.test".to_owned(),
                        upstreams: vec![bad.to_owned()],
                    }],
                },
            );
            assert!(validate(&config).is_err(), "should reject upstream {bad:?}");
        }
    }

    #[test]
    fn coredns_forward_valid_upstream_passes() {
        let cases = [
            vec!["10.0.0.1"],
            vec!["10.0.0.1:53"],
            vec!["dns.server:53"],
            vec!["my-resolver.internal"],
            vec!["10.0.0.1", "dns.server:53"],
            // RFC 1123 hostnames may begin with a digit.
            vec!["0.pool.ntp.org"],
            vec!["1dot1dot1dot1.cloudflare-dns.com:53"],
        ];
        for upstreams in cases {
            let mut config = base_config();
            config.spec.network = Some(NetworkConfig {
                cross_cluster: true,
                dns_zone: None,
            });
            config.spec.stacks.insert(
                "net".to_owned(),
                StackSpec {
                    description: None,
                    steps: vec![StepSpec::CoreDnsForward {
                        zone: "forge.test".to_owned(),
                        upstreams: upstreams.iter().map(|name| (*name).to_owned()).collect(),
                    }],
                },
            );
            validate(&config).unwrap_or_else(|_e| {
                std::process::abort();
            });
        }
    }

    #[test]
    fn cross_cluster_with_podman_rejected() {
        let mut config = base_config();
        config.spec.runtime = RuntimeConfig {
            provider: RuntimeProvider::Podman,
            ..RuntimeConfig::default()
        };
        config.spec.network = Some(NetworkConfig {
            cross_cluster: true,
            dns_zone: None,
        });
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("Podman"), "should mention Podman: {msg}");
    }

    #[test]
    fn cross_cluster_with_docker_passes() {
        let mut config = base_config();
        config.spec.runtime = RuntimeConfig {
            provider: RuntimeProvider::Docker,
            ..RuntimeConfig::default()
        };
        config.spec.network = Some(NetworkConfig {
            cross_cluster: true,
            dns_zone: None,
        });
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn cross_cluster_with_auto_passes() {
        let mut config = base_config();
        config.spec.network = Some(NetworkConfig {
            cross_cluster: true,
            dns_zone: None,
        });
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn capture_step_rejects_blank_resource() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Capture {
                    resource: String::new(),
                    namespace: None,
                    jsonpath: "{.spec}".to_owned(),
                    key: "k".to_owned(),
                    timeout: "1s".to_owned(),
                    interval: "1ms".to_owned(),
                }],
            },
        );
        assert!(validate(&config).is_err(), "capture should reject blank resource");
    }

    #[test]
    fn capture_step_rejects_blank_jsonpath() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Capture {
                    resource: "svc/web".to_owned(),
                    namespace: None,
                    jsonpath: String::new(),
                    key: "k".to_owned(),
                    timeout: "1s".to_owned(),
                    interval: "1ms".to_owned(),
                }],
            },
        );
        assert!(validate(&config).is_err(), "capture should reject blank jsonpath");
    }

    #[test]
    fn capture_step_rejects_key_with_dots() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Capture {
                    resource: "svc/web".to_owned(),
                    namespace: None,
                    jsonpath: "{.spec}".to_owned(),
                    key: "bad.key".to_owned(),
                    timeout: "1s".to_owned(),
                    interval: "1ms".to_owned(),
                }],
            },
        );
        assert!(validate(&config).is_err(), "capture should reject key with dots");
    }

    #[test]
    fn capture_step_valid_passes() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::Capture {
                    resource: "svc/provider-gateway".to_owned(),
                    namespace: Some("grid-system".to_owned()),
                    jsonpath: "{.status.loadBalancer.ingress[0].ip}".to_owned(),
                    key: "provider-gateway-ip".to_owned(),
                    timeout: "1s".to_owned(),
                    interval: "1ms".to_owned(),
                }],
            },
        );
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }

    #[test]
    fn template_manifest_validates_path() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::TemplateManifest {
                    path: "../escape.yaml".to_owned(),
                }],
            },
        );
        let Err(err) = validate(&config) else {
            std::process::abort();
        };
        let msg = err.to_string();
        assert!(msg.contains("escape"), "should reject path escape: {msg}");
    }

    #[test]
    fn template_manifest_valid_passes() {
        let mut config = base_config();
        config.spec.stacks.insert(
            "base".to_owned(),
            StackSpec {
                description: None,
                steps: vec![StepSpec::TemplateManifest {
                    path: "resources/gridnetwork.yaml".to_owned(),
                }],
            },
        );
        validate(&config).unwrap_or_else(|_e| {
            std::process::abort();
        });
    }
}
