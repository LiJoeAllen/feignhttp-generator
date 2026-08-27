use std::collections::BTreeMap;

use crate::ir::Operation;
use crate::naming::{sanitize_ident, sanitize_mod_name, sanitize_type_name};

/// An operation after applying the path-to-module mapping rules.
pub struct MappedOperation {
    /// Directory chain (snake_case module segments) leading to the file module.
    pub dirs: Vec<String>,
    /// File/module stem the operation lands in (`index` for short prefixes).
    pub module: String,
    /// Trait name (UpperCamel), derived from the raw module segment.
    pub trait_name: String,
    /// Final method name (snake_case, disambiguated).
    pub fn_name: String,
    pub op: Operation,
}

impl MappedOperation {
    /// Full Rust path of the containing module, e.g. `a::b` or `device`.
    pub fn mod_path(&self) -> String {
        let mut segs = self.dirs.clone();
        segs.push(self.module.clone());
        segs.join("::")
    }

    /// Raw doc line describing where this operation came from.
    pub fn origin(&self) -> String {
        format!(
            "{} {}",
            self.op.method.as_str().to_uppercase(),
            self.op.path
        )
    }
}

/// Map all operations onto the module tree.
///
/// Rules (plan §5 + Q6):
/// - Truncate at the first placeholder segment `{...}`; drop it and everything after.
/// - Standard mapping on the remaining prefix:
///   - >=2 segments: last = method, second-to-last = file/trait, rest = directories.
///   - ==1 segment: file/trait = `index`.
///   - ==0 segments: file/trait = `index`, method derived from operationId or verb.
/// - If a method base name within one file is claimed by multiple HTTP verbs,
///   ALL claimants are renamed `{verb}_{base}`; single-verb names stay bare.
///   Remaining identical names get a numeric suffix `_2`, `_3`, ...
pub fn map_operations(operations: Vec<Operation>) -> Result<Vec<MappedOperation>, String> {
    let mut mapped = Vec::with_capacity(operations.len());
    for op in operations {
        let (dirs_raw, module_raw, fn_base_raw) = split_path(&op.path);

        let (module_raw, fn_base) = match (dirs_raw.len(), module_raw) {
            (0, None) => (
                "index".to_string(),
                op.operation_id
                    .clone()
                    .unwrap_or_else(|| format!("{}_root", op.method.as_str())),
            ),
            (_, mod_opt) => (
                mod_opt.unwrap_or_else(|| "index".to_string()),
                match fn_base_raw {
                    Some(seg) => sanitize_ident(&seg),
                    None => op
                        .operation_id
                        .clone()
                        .unwrap_or_else(|| format!("{}_root", op.method.as_str())),
                },
            ),
        };

        let dirs: Vec<String> = dirs_raw.iter().map(|s| sanitize_mod_name(s)).collect();
        let is_reserved_top = dirs.is_empty() && module_raw == "models";
        let trait_name = sanitize_type_name(&module_raw);
        let module = if is_reserved_top {
            // Reserved: top-level `models` module holds generated types.
            "models_".to_string()
        } else {
            sanitize_mod_name(&module_raw)
        };

        mapped.push(MappedOperation {
            dirs,
            module,
            trait_name,
            fn_name: fn_base,
            op,
        });
    }

    disambiguate(&mut mapped);
    Ok(mapped)
}

/// Split a raw path into (directories, module stem, method base).
/// Returns all-None when nothing remains before the first placeholder.
fn split_path(path: &str) -> (Vec<String>, Option<String>, Option<String>) {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    let segs: Vec<&str> = clean.split('/').filter(|s| !s.is_empty()).collect();
    let end = segs
        .iter()
        .position(|s| s.contains('{') || s.contains('}'))
        .unwrap_or(segs.len());
    let kept = &segs[..end];
    match kept.len() {
        0 => (Vec::new(), None, None),
        1 => (
            Vec::new(),
            Some("index".to_string()),
            Some(kept[0].to_string()),
        ),
        n => (
            kept[..n - 2].iter().map(|s| s.to_string()).collect(),
            Some(kept[n - 2].to_string()),
            Some(kept[n - 1].to_string()),
        ),
    }
}

fn disambiguate(ops: &mut [MappedOperation]) {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, m) in ops.iter().enumerate() {
        groups.entry(m.mod_path()).or_default().push(i);
    }

    for indices in groups.values() {
        // Pass 1: how many operations share each fn base name? (owned keys so
        // later mutation of `ops` cannot conflict with borrows)
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for &i in indices {
            *counts.entry(ops[i].fn_name.clone()).or_default() += 1;
        }
        // Pass 2: multi-verb claimants -> {verb}_{base}.
        for &i in indices {
            if counts[&ops[i].fn_name] > 1 {
                ops[i].fn_name = format!("{}_{}", ops[i].op.method.as_str(), ops[i].fn_name);
            }
        }
        // Pass 3: any remaining exact duplicates get numeric suffixes.
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for &i in indices {
            let slot = seen.entry(ops[i].fn_name.clone()).or_insert(0);
            *slot += 1;
            if *slot > 1 {
                ops[i].fn_name = format!("{}_{}", ops[i].fn_name, slot);
            }
        }
    }
}
