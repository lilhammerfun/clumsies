use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use sha2::{Digest, Sha256};

use super::models::SearchModels;
use super::{RetrievalUnit, SearchFailure, SourceLocator, SourceResource};

const TARGET_TOKENS: usize = 384;
const HARD_TOKEN_LIMIT: usize = 480;
const TOKEN_OVERLAP: usize = 48;

#[derive(Clone, Debug)]
struct Heading {
    level: u8,
    start: usize,
    path: Vec<String>,
    occurrence: usize,
}

#[derive(Clone, Debug)]
struct Section {
    range: Range<usize>,
    heading_path: Vec<String>,
    identity: String,
    occurrence: usize,
}

struct UnitDescriptor<'a> {
    heading_path: Vec<String>,
    identity: &'a str,
    occurrence: usize,
    part_index: usize,
    range: Range<usize>,
    token_count: usize,
}

pub(crate) fn build_units(
    resource: &SourceResource,
    models: &dyn SearchModels,
) -> Result<Vec<RetrievalUnit>, SearchFailure> {
    let content = resource.content.as_str();
    let mut units = Vec::new();
    if !resource.description.trim().is_empty() {
        // The description is an explicit retrieval field: it is indexed as
        // its own unit so BM25, dense vectors and the reranker can match on
        // the agent-generated summary independently of the body text.
        let description = resource.description.trim();
        let token_count = models.token_offsets(description)?.len();
        units.push(RetrievalUnit {
            unit_key: format!("{}/description", resource.resource_id),
            resource_id: resource.resource_id.clone(),
            ordinal: 0,
            heading_path: Vec::new(),
            locator: SourceLocator::MarkdownSpan {
                start_byte: 0,
                end_byte: 0,
                heading_path: Vec::new(),
            },
            text: description.to_owned(),
            text_hash: sha256(description),
            token_count,
        });
    }
    if content.trim().is_empty() {
        return Ok(units);
    }
    let whole_offsets = models.token_offsets(content)?;
    if whole_offsets.len() <= TARGET_TOKENS {
        units.push(make_unit(
            resource,
            units.len(),
            UnitDescriptor {
                heading_path: Vec::new(),
                identity: "root",
                occurrence: 0,
                part_index: 0,
                range: trim_range(content, 0..content.len()),
                token_count: whole_offsets.len(),
            },
        ));
        return Ok(units);
    }

    for section in markdown_sections(content) {
        let range = trim_range(content, section.range);
        if range.is_empty() {
            continue;
        }
        let token_offsets = token_offsets_in_range(&whole_offsets, &range);
        let parts = if token_offsets.len() <= HARD_TOKEN_LIMIT {
            vec![(range, token_offsets.len())]
        } else {
            split_oversized_span(content, range, &whole_offsets)
        };
        for (part_index, (part_range, token_count)) in parts.into_iter().enumerate() {
            units.push(make_unit(
                resource,
                units.len(),
                UnitDescriptor {
                    heading_path: section.heading_path.clone(),
                    identity: &section.identity,
                    occurrence: section.occurrence,
                    part_index,
                    range: part_range,
                    token_count,
                },
            ));
        }
    }
    Ok(units)
}

fn make_unit(
    resource: &SourceResource,
    ordinal: usize,
    descriptor: UnitDescriptor<'_>,
) -> RetrievalUnit {
    let UnitDescriptor {
        heading_path,
        identity,
        occurrence,
        part_index,
        range,
        token_count,
    } = descriptor;
    let text = resource.content[range.clone()].to_owned();
    let text_hash = sha256(&text);
    RetrievalUnit {
        unit_key: format!(
            "{}/{identity}/{occurrence}/{part_index}",
            resource.resource_id
        ),
        resource_id: resource.resource_id.clone(),
        ordinal,
        heading_path: heading_path.clone(),
        locator: SourceLocator::MarkdownSpan {
            start_byte: range.start,
            end_byte: range.end,
            heading_path,
        },
        text,
        text_hash,
        token_count,
    }
}

fn markdown_sections(content: &str) -> Vec<Section> {
    let headings = parse_headings(content);
    if headings.is_empty() {
        return vec![Section {
            range: 0..content.len(),
            heading_path: Vec::new(),
            identity: "root".to_owned(),
            occurrence: 0,
        }];
    }

    let mut sections = Vec::new();
    if !content[..headings[0].start].trim().is_empty() {
        sections.push(Section {
            range: 0..headings[0].start,
            heading_path: Vec::new(),
            identity: "root".to_owned(),
            occurrence: 0,
        });
    }
    for (index, heading) in headings.iter().enumerate() {
        let end = headings[index + 1..]
            .iter()
            .find(|candidate| candidate.level <= heading.level)
            .map(|candidate| candidate.start)
            .unwrap_or(content.len());
        sections.push(Section {
            range: heading.start..end,
            heading_path: heading.path.clone(),
            identity: heading
                .path
                .iter()
                .map(|component| normalize_identity(component))
                .collect::<Vec<_>>()
                .join("/"),
            occurrence: heading.occurrence,
        });
    }
    sections
}

fn parse_headings(content: &str) -> Vec<Heading> {
    let parser = Parser::new_ext(content, Options::all()).into_offset_iter();
    let mut raw = Vec::<(u8, usize, String)>::new();
    let mut active: Option<(u8, usize, String)> = None;
    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                active = Some((heading_level(level), range.start, String::new()));
            }
            Event::Text(text) | Event::Code(text) if active.is_some() => {
                let (_, _, title) = active.as_mut().expect("checked above");
                if !title.is_empty() {
                    title.push(' ');
                }
                title.push_str(&text);
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, start, title)) = active.take() {
                    raw.push((level, start, collapse_whitespace(&title)));
                }
            }
            _ => {}
        }
    }

    let mut stack = Vec::<(u8, String)>::new();
    let mut occurrences = HashMap::<String, usize>::new();
    raw.into_iter()
        .map(|(level, start, title)| {
            while stack.last().is_some_and(|(parent, _)| *parent >= level) {
                stack.pop();
            }
            let title = if title.is_empty() {
                "Untitled".to_owned()
            } else {
                title
            };
            stack.push((level, title.clone()));
            let path = stack
                .iter()
                .map(|(_, component)| component.clone())
                .collect::<Vec<_>>();
            let identity = path
                .iter()
                .map(|component| normalize_identity(component))
                .collect::<Vec<_>>()
                .join("/");
            let occurrence = occurrences.entry(identity).or_default();
            let current = *occurrence;
            *occurrence += 1;
            Heading {
                level,
                start,
                path,
                occurrence: current,
            }
        })
        .collect()
}

fn split_oversized_span(
    content: &str,
    range: Range<usize>,
    token_offsets: &[(usize, usize)],
) -> Vec<(Range<usize>, usize)> {
    let slice = &content[range.clone()];
    let boundaries = markdown_block_boundaries(slice);
    let mut parts = Vec::<Range<usize>>::new();
    let mut start = 0usize;
    let mut cursor = 0usize;
    for boundary in boundaries {
        let candidate = trim_range(slice, start..boundary);
        let candidate = (range.start + candidate.start)..(range.start + candidate.end);
        let count = token_offsets_in_range(token_offsets, &candidate).len();
        if count > TARGET_TOKENS && cursor > start {
            let committed = trim_range(slice, start..cursor);
            if !committed.is_empty() {
                parts.push(committed);
            }
            start = cursor;
        }
        cursor = boundary;
    }
    let tail = trim_range(slice, start..slice.len());
    if !tail.is_empty() {
        parts.push(tail);
    }

    let mut output = Vec::new();
    for part in parts {
        let part = (range.start + part.start)..(range.start + part.end);
        let offsets = token_offsets_in_range(token_offsets, &part);
        if offsets.len() <= HARD_TOKEN_LIMIT {
            output.push((part, offsets.len()));
            continue;
        }
        for (window, count) in token_windows(content, part, offsets) {
            output.push((window, count));
        }
    }
    output
}

fn token_offsets_in_range<'a>(
    offsets: &'a [(usize, usize)],
    range: &Range<usize>,
) -> &'a [(usize, usize)] {
    if range.is_empty() {
        return &offsets[..0];
    }
    let first = offsets.partition_point(|(_, end)| *end <= range.start);
    let remaining = &offsets[first..];
    let count = remaining.partition_point(|(start, _)| *start < range.end);
    &remaining[..count]
}

fn markdown_block_boundaries(content: &str) -> Vec<usize> {
    let mut boundaries = Parser::new_ext(content, Options::all())
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::End(
                TagEnd::Paragraph
                | TagEnd::CodeBlock
                | TagEnd::List(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::Table,
            ) => Some(range.end),
            _ => None,
        })
        .collect::<Vec<_>>();
    boundaries.push(content.len());
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
}

fn token_windows(
    content: &str,
    span: Range<usize>,
    offsets: &[(usize, usize)],
) -> Vec<(Range<usize>, usize)> {
    let mut windows = Vec::new();
    let mut token_start = 0usize;
    while token_start < offsets.len() {
        let token_end = (token_start + TARGET_TOKENS).min(offsets.len());
        let byte_start = if token_start == 0 {
            span.start
        } else {
            offsets[token_start].0.max(span.start)
        };
        let byte_end = if token_end == offsets.len() {
            span.end
        } else {
            offsets[token_end - 1].1.min(span.end)
        };
        let range = trim_range(content, byte_start..byte_end);
        if !range.is_empty() {
            windows.push((range, token_end - token_start));
        }
        if token_end == offsets.len() {
            break;
        }
        token_start = token_end.saturating_sub(TOKEN_OVERLAP);
    }
    windows
}

fn trim_range(content: &str, mut range: Range<usize>) -> Range<usize> {
    while range.start < range.end {
        let Some(character) = content[range.start..range.end].chars().next() else {
            break;
        };
        if !character.is_whitespace() {
            break;
        }
        range.start += character.len_utf8();
    }
    while range.start < range.end {
        let Some(character) = content[range.start..range.end].chars().next_back() else {
            break;
        };
        if !character.is_whitespace() {
            break;
        }
        range.end -= character.len_utf8();
    }
    range
}

fn normalize_identity(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in value.trim().to_lowercase().chars() {
        if character.is_alphanumeric() || character == '_' {
            if separator && !output.is_empty() {
                output.push('-');
            }
            output.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    if output.is_empty() {
        "section".to_owned()
    } else {
        output
    }
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn sha256(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::search::models::SearchModelRuntimeStatus;
    use crate::search::{MemoryKind, SourceScope};

    struct CharacterModels;

    #[derive(Default)]
    struct CountingModels {
        tokenization_calls: AtomicUsize,
        tokenized_bytes: AtomicUsize,
    }

    impl SearchModels for CharacterModels {
        fn revision(&self) -> Result<String, SearchFailure> {
            Ok("test".to_owned())
        }

        fn token_offsets(&self, text: &str) -> Result<Vec<(usize, usize)>, SearchFailure> {
            Ok(text
                .char_indices()
                .map(|(start, character)| (start, start + character.len_utf8()))
                .collect())
        }

        fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SearchFailure> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }

        fn embed_query(&self, _query: &str) -> Result<Vec<f32>, SearchFailure> {
            Ok(vec![1.0, 0.0])
        }

        fn rerank(&self, _query: &str, documents: &[String]) -> Result<Vec<f32>, SearchFailure> {
            Ok((0..documents.len()).map(|index| -(index as f32)).collect())
        }

        fn dimensions(&self) -> usize {
            2
        }

        fn status(&self) -> SearchModelRuntimeStatus {
            SearchModelRuntimeStatus::Ready
        }
    }

    impl SearchModels for CountingModels {
        fn revision(&self) -> Result<String, SearchFailure> {
            Ok("counting-test".to_owned())
        }

        fn token_offsets(&self, text: &str) -> Result<Vec<(usize, usize)>, SearchFailure> {
            self.tokenization_calls.fetch_add(1, Ordering::Relaxed);
            self.tokenized_bytes
                .fetch_add(text.len(), Ordering::Relaxed);
            CharacterModels.token_offsets(text)
        }

        fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SearchFailure> {
            CharacterModels.embed_passages(texts)
        }

        fn embed_query(&self, query: &str) -> Result<Vec<f32>, SearchFailure> {
            CharacterModels.embed_query(query)
        }

        fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>, SearchFailure> {
            CharacterModels.rerank(query, documents)
        }

        fn dimensions(&self) -> usize {
            CharacterModels.dimensions()
        }

        fn status(&self) -> SearchModelRuntimeStatus {
            CharacterModels.status()
        }
    }

    fn resource(content: String) -> SourceResource {
        resource_with_description(content, String::new())
    }

    fn resource_with_description(content: String, description: String) -> SourceResource {
        SourceResource {
            resource_id: "ctx_test".to_owned(),
            project_id: "prj_test".to_owned(),
            scope: SourceScope::Project,
            kind: MemoryKind::Memory,
            path: "architecture/search.md".to_owned(),
            title: "Search".to_owned(),
            description,
            content_hash: sha256(&content),
            content,
            source_commit_id: Some("commit_test".to_owned()),
            draft_id: None,
            draft_revision: None,
        }
    }

    #[test]
    fn short_documents_remain_one_root_unit() {
        let units = build_units(
            &resource("# One\n\nBody\n\n## Two\n\nMore".to_owned()),
            &CharacterModels,
        )
        .unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].unit_key, "ctx_test/root/0/0");
        assert_eq!(units[0].locator.start_byte(), 0);
    }

    #[test]
    fn long_documents_keep_heading_paths_and_utf8_byte_ranges() {
        let content = format!(
            "前言{}\n\n# 检索\n\n{}\n\n## Delta\n\n{}",
            "甲".repeat(400),
            "乙".repeat(420),
            "丙".repeat(520)
        );
        let units = build_units(&resource(content.clone()), &CharacterModels).unwrap();
        assert!(units.len() >= 4);
        assert!(
            units
                .iter()
                .any(|unit| unit.heading_path == ["检索", "Delta"])
        );
        for unit in units {
            let SourceLocator::MarkdownSpan {
                start_byte,
                end_byte,
                ..
            } = unit.locator;
            assert!(content.is_char_boundary(start_byte));
            assert!(content.is_char_boundary(end_byte));
            assert_eq!(unit.text, content[start_byte..end_byte]);
        }
    }

    #[test]
    fn duplicate_headings_receive_stable_occurrences() {
        let body = "x".repeat(500);
        let content = format!("# A\n{body}\n# A\n{body}");
        let units = build_units(&resource(content), &CharacterModels).unwrap();
        assert!(units.iter().any(|unit| unit.unit_key.contains("/a/0/")));
        assert!(units.iter().any(|unit| unit.unit_key.contains("/a/1/")));
    }

    #[test]
    fn many_markdown_blocks_reuse_the_resource_token_offsets() {
        let content = (0..2_048)
            .map(|index| format!("paragraph-{index} {}\n\n", "x".repeat(64)))
            .collect::<String>();
        let models = CountingModels::default();

        let units = build_units(&resource(content.clone()), &models).unwrap();

        assert!(units.len() > 100);
        assert!(
            units
                .iter()
                .all(|unit| unit.token_count <= HARD_TOKEN_LIMIT)
        );
        assert_eq!(models.tokenization_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            models.tokenized_bytes.load(Ordering::Relaxed),
            content.len()
        );
    }

    #[test]
    fn test_model_is_send_and_sync() {
        let _: Arc<dyn SearchModels> = Arc::new(CharacterModels);
    }

    #[test]
    fn description_is_indexed_as_its_own_retrieval_unit() {
        let units = build_units(
            &resource_with_description(
                "# Body\n\nImplementation details only.\n".to_owned(),
                "Deployment runbook: how to ship releases safely".to_owned(),
            ),
            &CharacterModels,
        )
        .unwrap();

        let description_unit = units
            .iter()
            .find(|unit| unit.unit_key.ends_with("/description"))
            .expect("description unit exists");
        assert_eq!(
            description_unit.text,
            "Deployment runbook: how to ship releases safely"
        );
        // The body unit still points at the real content range.
        let body_unit = units
            .iter()
            .find(|unit| !unit.unit_key.ends_with("/description"))
            .expect("body unit exists");
        assert!(!body_unit.text.contains("Deployment runbook"));
        // Querying on description-only vocabulary reaches the resource.
        let needle = "ship releases safely";
        assert!(
            units.iter().any(|unit| unit.text.contains(needle)),
            "description vocabulary must be retrievable"
        );
    }
}
