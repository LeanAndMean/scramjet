//! YAML documents loaded with their aliases bounded. `yaml-rust2` copies an anchored value at
//! each alias, so a few nested aliases in a small file would expand into billions of nodes;
//! Scramjet's `yaml` refuses such documents too ("Excessive alias count").

use std::collections::HashMap;
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::Marker;
use yaml_rust2::{Yaml, YamlLoader};

/// The most nodes aliases may add to a document, counted as copied.
const ALIASED_NODES: usize = 100_000;

/// The first document of `text`, `Null` when there is none.
pub(super) fn first(text: &str) -> Result<Yaml, String> {
    let mut size = Size::default();
    Parser::new_from_str(text)
        .load(&mut size, true)
        .map_err(|error| error.to_string())?;
    if size.aliased > ALIASED_NODES {
        return Err(format!(
            "its aliases expand to over {ALIASED_NODES} nodes, which suggests a resource exhaustion attack"
        ));
    }
    let documents = YamlLoader::load_from_str(text).map_err(|error| error.to_string())?;
    Ok(documents.into_iter().next().unwrap_or(Yaml::Null))
}

/// Counts the nodes each anchor holds and those its aliases copy.
#[derive(Default)]
struct Size {
    open: Vec<(usize, usize)>,
    anchors: HashMap<usize, usize>,
    aliased: usize,
}

impl MarkedEventReceiver for Size {
    fn on_event(&mut self, event: Event, _: Marker) {
        let (anchor, nodes) = match event {
            Event::SequenceStart(anchor, _) | Event::MappingStart(anchor, _) => {
                self.open.push((anchor, 1));
                return;
            }
            Event::SequenceEnd | Event::MappingEnd => self.open.pop().unwrap_or_default(),
            Event::Scalar(_, _, anchor, _) => (anchor, 1),
            Event::Alias(id) => {
                let nodes = self.anchors.get(&id).copied().unwrap_or(1);
                self.aliased = self.aliased.saturating_add(nodes);
                (0, nodes)
            }
            _ => return,
        };
        if anchor > 0 {
            self.anchors.insert(anchor, nodes);
        }
        if let Some(parent) = self.open.last_mut() {
            parent.1 = parent.1.saturating_add(nodes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_load_as_copies_until_they_would_expand_too_far() {
        let document = first("base: &b {x: 1}\nuse: *b\nlist: [*b, *b]\n").unwrap();
        assert_eq!(document["use"]["x"].as_i64(), Some(1));
        assert_eq!(document["list"][1]["x"].as_i64(), Some(1));
        assert_eq!(first("").unwrap(), Yaml::Null);
        assert!(first("a: [").is_err());

        let mut bomb = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
        for level in 1..30 {
            let previous = format!("*a{}", level - 1);
            let items = [previous.as_str(); 10].join(", ");
            bomb.push_str(&format!("a{level}: &a{level} [{items}]\n"));
        }
        let started = std::time::Instant::now();
        let error = first(&bomb).unwrap_err();
        assert!(error.contains("resource exhaustion"), "{error}");
        assert!(started.elapsed().as_secs() < 2);

        let mut wide = String::from("base: &b [1, 2, 3, 4, 5, 6, 7, 8, 9]\nuses:\n");
        for _ in 0..1000 {
            wide.push_str("  - *b\n");
        }
        assert_eq!(first(&wide).unwrap()["uses"][999][8].as_i64(), Some(9));
    }
}
