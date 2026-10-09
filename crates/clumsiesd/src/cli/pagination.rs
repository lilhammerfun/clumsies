//! CLI page parameters and explicit traversal of local and Server collections.

use clap::Args;
use serde_json::Value;
use std::collections::BTreeSet;

/// Shared page controls; all-pages traversal starts at the first page.
#[derive(Args, Clone, Debug)]
pub(super) struct PageArgs {
    /// Maximum items per request (1-200).
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=200))]
    pub limit: u16,
    /// Opaque continuation cursor from the previous response.
    #[arg(long, conflicts_with = "all")]
    pub cursor: Option<String>,
    /// Fetch all pages, failing without output if any request fails.
    #[arg(long)]
    pub all: bool,
}

impl PageArgs {
    /// Adds encoded page parameters without interpreting the cursor.
    pub fn path(&self, path: &str, cursor: Option<&str>) -> String {
        let mut url = reqwest::Url::parse("http://localhost/").expect("constant origin");
        let mut query = url.query_pairs_mut();
        query.append_pair("limit", &self.limit.to_string());
        if let Some(cursor) = cursor {
            query.append_pair("cursor", cursor);
        }
        format!(
            "{path}{}{}",
            if path.contains('?') { '&' } else { '?' },
            query.finish().query().expect("page parameters")
        )
    }
}

/// Fetches one page or combines all pages while retaining the collection's response shape.
///
/// # Errors
/// Propagates request failures and rejects malformed or looping continuation metadata.
pub(super) fn collect(
    args: &PageArgs,
    mut fetch: impl FnMut(Option<&str>) -> Result<Value, Box<dyn std::error::Error>>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut cursor = args.cursor.clone();
    let mut seen = BTreeSet::new();
    let mut result: Option<Value> = None;
    loop {
        let mut page = fetch(cursor.as_deref())?;
        let next = continuation(&page)?;
        let items = page["items"]
            .as_array_mut()
            .ok_or("List response requires an items array")?;
        if let Some(result) = &mut result {
            result["items"].as_array_mut().unwrap().append(items);
        } else {
            result = Some(page.clone());
        }
        if !args.all || next.is_none() {
            let result = result.as_mut().unwrap();
            if page.get("page_info").is_some() {
                result["page_info"] = page["page_info"].clone();
            } else {
                result["next_cursor"] = page["next_cursor"].clone();
            }
            return Ok(result.take());
        }
        let next = next.unwrap();
        if !seen.insert(next.clone()) || cursor.as_ref() == Some(&next) {
            return Err(
                "Pagination returned a repeated cursor; no partial result was printed".into(),
            );
        }
        cursor = Some(next);
    }
}

/// Prints JSON using the existing page contract, or streams every page for human reading.
///
/// # Errors
/// Propagates malformed or repeating cursors, incomplete reads, and reader cancellation.
pub(super) fn print(
    args: &PageArgs,
    output: &mut super::output::Output,
    mut fetch: impl FnMut(Option<&str>) -> Result<Value, Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if output.json {
        return output.value(&collect(args, fetch)?);
    }
    let mut cursor = args.cursor.clone();
    let mut seen = BTreeSet::new();
    let mut first = true;
    let mut count = 0;
    loop {
        let page = fetch(cursor.as_deref())
            .map_err(|error| format!("List incomplete after {count} results: {error}"))?;
        let next = continuation(&page)?;
        output.page(&page, first)?;
        count += page["items"]
            .as_array()
            .ok_or("List response requires items")?
            .len();
        first = false;
        let Some(next) = next else {
            break;
        };
        if !seen.insert(next.clone()) || cursor.as_ref() == Some(&next) {
            return Err("List incomplete: repeated continuation cursor".into());
        }
        cursor = Some(next);
    }
    if count == 0 {
        output.text("No results.\n")?;
    }
    Ok(())
}

/// Decodes the two existing collection envelopes and checks continuation consistency.
///
/// # Errors
/// Rejects missing metadata, invalid cursor types, or has_more without a continuation.
fn continuation(page: &Value) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let metadata = page.get("page_info").unwrap_or(page);
    let next = metadata
        .get("next_cursor")
        .ok_or("List response requires next_cursor")?;
    let next = match next {
        Value::Null => None,
        Value::String(next) if !next.is_empty() => Some(next.clone()),
        _ => return Err("List response contains an invalid cursor".into()),
    };
    if page.get("page_info").is_some() && metadata["has_more"].as_bool() != Some(next.is_some()) {
        return Err("List response has inconsistent has_more and next_cursor".into());
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn all_pages_follow_opaque_cursors_and_keep_terminal_metadata() {
        let args = PageArgs {
            limit: 1,
            cursor: None,
            all: true,
        };
        let mut calls = Vec::new();
        let output = collect(&args, |cursor| {
            calls.push(cursor.map(str::to_owned));
            Ok(if cursor.is_none() {
                json!({"items":[1],"page_info":{"has_more":true,"next_cursor":"a+/&"}})
            } else {
                json!({"items":[2],"page_info":{"has_more":false,"next_cursor":null}})
            })
        })
        .unwrap();
        assert_eq!(calls, vec![None, Some("a+/&".to_owned())]);
        assert_eq!(output["items"], json!([1, 2]));
        assert_eq!(output["page_info"]["has_more"], false);
        assert_eq!(
            args.path("/reviews?project_id=p", Some("a+/&")),
            "/reviews?project_id=p&limit=1&cursor=a%2B%2F%26"
        );
    }

    #[test]
    fn one_page_does_not_follow_and_all_pages_reject_loops_and_failures() {
        let mut args = PageArgs {
            limit: 1,
            cursor: Some("start".into()),
            all: false,
        };
        let page = collect(&args, |cursor| {
            assert_eq!(cursor, Some("start"));
            Ok(json!({"items":[],"next_cursor":"next"}))
        })
        .unwrap();
        assert_eq!(page["next_cursor"], "next");
        args.all = true;
        args.cursor = None;
        assert!(collect(&args, |_| Ok(json!({"items":[1],"next_cursor":"same"}))).is_err());
        assert!(
            collect(&args, |cursor| if cursor.is_none() {
                Ok(json!({"items":[1],"next_cursor":"next"}))
            } else {
                Err("network failure".into())
            })
            .is_err()
        );
        assert!(
            collect(&args, |_| Ok(
                json!({"items":[],"page_info":{"has_more":true,"next_cursor":null}})
            ))
            .is_err()
        );
    }
}
