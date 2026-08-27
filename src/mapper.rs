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
        format!("{} {}", self.op.method.as_str().to_uppercase(), self.op.path)
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
            (0, None) => {
                ("index".to_string(), op.operation_id.clone().unwrap_or_else(|| format!("{}_root", op.method.as_str())))
            }
            (_, mod_opt) => (
                mod_opt.unwrap_or_else(|| "index".to_string()),
                match fn_base_raw {
                    Some(seg) => sanitize_ident(&seg),
                    None => op.operation_id.clone().unwrap_or_else(|| format!("{}_root", op.method.as_str())),
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

        mapped.push(MappedOperation { dirs, module, trait_name, fn_name: fn_base, op });
    }

    disambiguate(&mut mapped);
    Ok(mapped)
}

/// Split a raw path into (directories, module stem, method base).
/// Returns all-None when nothing remains before the first placeholder.
fn split_path(path: &str) -> (Vec<String>, Option<String>, Option<String>) {
    let clean = path.split(['?', '#']).next().unwrap_or(path);
    let segs: Vec<&str> = clean.split('/').filter(|s| !s.is_empty()).collect();
    let end = segs.iter().position(|s| s.contains('{') || s.contains('}')).unwrap_or(segs.len());
    let kept = &segs[..end];
    match kept.len() {
        0 => (Vec::new(), None, None),
        1 => (Vec::new(), Some("index".to_string()), Some(kept[0].to_string())),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{HttpMethod, Operation};

    // ── split_path ──────────────────────────────────────────────────────

    #[test]
    fn split_typical_path() {
        // Truncated at first placeholder, only "device-groups" remains
        assert_eq!(
            split_path("/device-groups/{groupId}/devices/{deviceId}/status"),
            (vec![], Some("index".to_string()), Some("device-groups".to_string()))
        );
    }

    #[test]
    fn split_single_segment() {
        assert_eq!(split_path("/pets"), (vec![], Some("index".to_string()), Some("pets".to_string())));
    }

    #[test]
    fn split_empty_path() {
        assert_eq!(split_path("/"), (vec![], None, None));
    }

    #[test]
    fn split_path_with_query() {
        assert_eq!(split_path("/pets?page=1&limit=20"), (vec![], Some("index".to_string()), Some("pets".to_string())));
    }

    #[test]
    fn split_path_with_fragment() {
        assert_eq!(split_path("/pets#overview"), (vec![], Some("index".to_string()), Some("pets".to_string())));
    }

    #[test]
    fn split_only_placeholders() {
        assert_eq!(split_path("/{id}"), (vec![], None, None));
    }

    #[test]
    fn split_deep_no_placeholders() {
        assert_eq!(
            split_path("/a/b/c/d"),
            (vec!["a".to_string(), "b".to_string()], Some("c".to_string()), Some("d".to_string()))
        );
    }

    #[test]
    fn split_root_path() {
        assert_eq!(split_path(""), (vec![], None, None));
    }

    // ── map_operations ──────────────────────────────────────────────────

    fn make_op(method: &str, path: &str, operation_id: Option<&str>) -> Operation {
        Operation {
            method: HttpMethod::from_str(method).unwrap(),
            path: path.to_string(),
            operation_id: operation_id.map(String::from),
            summary: None,
            parameters: vec![],
            request_body: None,
            success: None,
            error_schemas: vec![],
            deprecated: false,
        }
    }

    #[test]
    fn map_single_operation() {
        // Path "/pets/{id}" → split_path returns ("index", "pets"), so fn_name = "pets"
        let ops = vec![make_op("get", "/pets/{id}", Some("get_pet"))];
        let mapped = map_operations(ops).unwrap();
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].mod_path(), "index");
        assert_eq!(mapped[0].fn_name, "pets");
    }

    #[test]
    fn map_operation_without_id_uses_path_segment() {
        // Path segment becomes the fn name even without operationId
        let ops = vec![make_op("get", "/pets/{id}", None)];
        let mapped = map_operations(ops).unwrap();
        assert_eq!(mapped[0].fn_name, "pets");
    }

    #[test]
    fn map_operation_id_used_when_no_path_segment() {
        // When the path is entirely placeholders, operationId is used
        let ops = vec![make_op("get", "/{id}", Some("get_pet"))];
        let mapped = map_operations(ops).unwrap();
        assert_eq!(mapped[0].mod_path(), "index");
        assert_eq!(mapped[0].fn_name, "get_pet");
    }

    #[test]
    fn map_operation_without_id_falls_back_to_verb_root() {
        // When path is all placeholders and no operationId, use {verb}_root
        let ops = vec![make_op("get", "/{id}", None)];
        let mapped = map_operations(ops).unwrap();
        assert_eq!(mapped[0].fn_name, "get_root");
    }

    #[test]
    fn map_operation_with_directory_chain() {
        // Path "/a/b/c" → split_path returns (["a"], "b", "c"), so mod_path = "a::b"
        let ops = vec![make_op("get", "/a/b/c", Some("get_c"))];
        let mapped = map_operations(ops).unwrap();
        assert_eq!(mapped[0].mod_path(), "a::b");
        assert_eq!(mapped[0].fn_name, "c");
    }

    #[test]
    fn map_operations_on_different_paths() {
        let ops =
            vec![make_op("get", "/pets/{id}", Some("get_pet")), make_op("post", "/store/order", Some("place_order"))];
        let mapped = map_operations(ops).unwrap();
        assert_eq!(mapped.len(), 2);
        let paths: Vec<String> = mapped.iter().map(|m| m.mod_path()).collect();
        assert!(paths.contains(&"index".to_string()));
        assert!(paths.contains(&"store".to_string()));
    }

    // ── disambiguate ────────────────────────────────────────────────────

    #[test]
    fn disambiguate_get_and_post_on_same_path() {
        let ops = vec![make_op("get", "/pets", None), make_op("post", "/pets", None)];
        let mapped = map_operations(ops).unwrap();
        assert_eq!(mapped.len(), 2);
        let fns: Vec<&str> = mapped.iter().map(|m| m.fn_name.as_str()).collect();
        assert!(fns.contains(&"get_pets"));
        assert!(fns.contains(&"post_pets"));
    }

    #[test]
    fn disambiguate_duplicate_operation_ids() {
        // Use placeholder-only path so operation_id is used as fn_name
        let ops = vec![make_op("get", "/{id}", Some("list")), make_op("post", "/{id}", Some("list"))];
        let mapped = map_operations(ops).unwrap();
        let fns: Vec<&str> = mapped.iter().map(|m| m.fn_name.as_str()).collect();
        assert!(fns.contains(&"get_list"));
        assert!(fns.contains(&"post_list"));
    }

    #[test]
    fn disambiguate_three_identical_names() {
        let ops = vec![make_op("get", "/pets", None), make_op("post", "/pets", None), make_op("put", "/pets", None)];
        let mapped = map_operations(ops).unwrap();
        assert_eq!(mapped.len(), 3);
        let fns: Vec<&str> = mapped.iter().map(|m| m.fn_name.as_str()).collect();
        assert!(fns.contains(&"get_pets"));
        assert!(fns.contains(&"post_pets"));
        assert!(fns.contains(&"put_pets"));
    }

    #[test]
    fn reserved_models_module_is_renamed() {
        // Use a path with >=2 segments so module_raw = "models"
        let ops = vec![make_op("get", "/models/something", Some("list"))];
        let mapped = map_operations(ops).unwrap();
        assert_eq!(mapped[0].module, "models_");
        assert_eq!(mapped[0].trait_name, "Models");
    }

    #[test]
    fn disambiguate_same_verb_gets_numeric_suffix() {
        // Use placeholder-only path so operation_id is used as fn_name.
        // Same verb + same operation_id → count > 1 → verb prefix added,
        // then numeric suffix on the second.
        let ops = vec![make_op("get", "/{id}", Some("list")), make_op("get", "/{id}", Some("list"))];
        let mapped = map_operations(ops).unwrap();
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].fn_name, "get_list");
        assert_eq!(mapped[1].fn_name, "get_list_2");
    }
}
