use anyhow::{Context, Result, bail};
use notes_ai::retrieval::RetrievalPipeline;
use notes_core::db::SearchHit;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::path::Path;

const EVAL_LIMIT: u32 = 10;
const MAX_QUERIES: usize = 500;
const MAX_EXPECTED_PER_QUERY: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenSet {
    pub query: Vec<GoldenQuery>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenQuery {
    pub text: String,
    pub expect: Vec<uuid::Uuid>,
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MetricSummary {
    pub queries: usize,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub mrr: f64,
}

pub struct EvalReport {
    pub overall: MetricSummary,
    pub groups: BTreeMap<String, MetricSummary>,
    pub queries: Vec<QueryResult>,
    pub rerank: bool,
}

pub struct QueryResult {
    pub text: String,
    pub group: String,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub reciprocal_rank: f64,
    pub missed_expected: Vec<uuid::Uuid>,
    pub hits: Vec<RankedHit>,
}

pub struct RankedHit {
    pub uuid: uuid::Uuid,
    pub score: f64,
    pub preview: String,
}

pub fn load_golden_set(path: &Path) -> Result<GoldenSet> {
    let input = std::fs::read_to_string(path)
        .with_context(|| format!("reading eval query set {}", path.display()))?;
    let set: GoldenSet = toml::from_str(&input)
        .with_context(|| format!("parsing eval query set {}", path.display()))?;
    validate_golden_set(&set)?;
    Ok(set)
}

fn validate_golden_set(set: &GoldenSet) -> Result<()> {
    if set.query.is_empty() {
        bail!("eval query set must contain at least one [[query]]");
    }
    if set.query.len() > MAX_QUERIES {
        bail!("eval query set cannot contain more than {MAX_QUERIES} queries");
    }
    for (index, query) in set.query.iter().enumerate() {
        if query.text.trim().is_empty() {
            bail!("query {} has empty text", index + 1);
        }
        if query.expect.is_empty() {
            bail!("query {} must expect at least one UUID", index + 1);
        }
        if query.expect.len() > MAX_EXPECTED_PER_QUERY {
            bail!(
                "query {} cannot expect more than {MAX_EXPECTED_PER_QUERY} UUIDs",
                index + 1
            );
        }
        if query.expect.iter().copied().collect::<HashSet<_>>().len() != query.expect.len() {
            bail!("query {} contains duplicate expected UUIDs", index + 1);
        }
        if query
            .group
            .as_deref()
            .is_some_and(|group| group.trim().is_empty())
        {
            bail!("query {} has an empty group", index + 1);
        }
    }
    Ok(())
}

pub async fn evaluate(
    pipeline: &RetrievalPipeline,
    set: GoldenSet,
    rerank: bool,
) -> Result<EvalReport> {
    let mut queries = Vec::with_capacity(set.query.len());
    for query in set.query {
        let hits = pipeline
            .retrieve_with_rerank(query.text.clone(), EVAL_LIMIT, rerank)
            .await
            .with_context(|| format!("evaluating query {:?}", query.text))?;
        queries.push(score_query(query, hits));
    }
    let overall = summarize(queries.iter());
    let mut grouped = BTreeMap::<String, Vec<&QueryResult>>::new();
    for query in &queries {
        grouped.entry(query.group.clone()).or_default().push(query);
    }
    let groups = grouped
        .into_iter()
        .map(|(group, queries)| (group, summarize(queries)))
        .collect();
    Ok(EvalReport {
        overall,
        groups,
        queries,
        rerank,
    })
}

fn score_query(query: GoldenQuery, hits: Vec<SearchHit>) -> QueryResult {
    let expected = query.expect.iter().copied().collect::<HashSet<_>>();
    let ranked = hits
        .into_iter()
        .map(|hit| RankedHit {
            uuid: hit.content.uuid(),
            score: hit.score,
            preview: preview(hit.content.text()),
        })
        .collect::<Vec<_>>();
    let recall_at = |limit: usize| {
        ranked
            .iter()
            .take(limit)
            .filter(|hit| expected.contains(&hit.uuid))
            .count() as f64
            / expected.len() as f64
    };
    let reciprocal_rank = ranked
        .iter()
        .take(EVAL_LIMIT as usize)
        .position(|hit| expected.contains(&hit.uuid))
        .map_or(0.0, |rank| 1.0 / (rank + 1) as f64);
    let returned = ranked.iter().map(|hit| hit.uuid).collect::<HashSet<_>>();
    let missed_expected = query
        .expect
        .iter()
        .copied()
        .filter(|uuid| !returned.contains(uuid))
        .collect();
    QueryResult {
        text: query.text,
        group: query
            .group
            .map(|group| group.trim().to_owned())
            .unwrap_or_else(|| "ungrouped".into()),
        recall_at_5: recall_at(5),
        recall_at_10: recall_at(10),
        reciprocal_rank,
        missed_expected,
        hits: ranked,
    }
}

fn summarize<'a>(queries: impl IntoIterator<Item = &'a QueryResult>) -> MetricSummary {
    let mut summary = MetricSummary::default();
    for query in queries {
        summary.queries += 1;
        summary.recall_at_5 += query.recall_at_5;
        summary.recall_at_10 += query.recall_at_10;
        summary.mrr += query.reciprocal_rank;
    }
    if summary.queries != 0 {
        let count = summary.queries as f64;
        summary.recall_at_5 /= count;
        summary.recall_at_10 /= count;
        summary.mrr /= count;
    }
    summary
}

fn preview(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= 80 {
        return normalized;
    }
    let prefix = normalized.chars().take(79).collect::<String>();
    format!("{prefix}…")
}

pub fn format_report(report: &EvalReport) -> String {
    let mut output = String::new();
    writeln!(
        output,
        "mode: {}",
        if report.rerank { "rerank" } else { "rrf-only" }
    )
    .unwrap();
    write_summary(&mut output, "overall", report.overall);
    writeln!(output, "groups:").unwrap();
    for (group, summary) in &report.groups {
        write_summary(&mut output, &format!("  {group}"), *summary);
    }
    writeln!(output, "queries:").unwrap();
    for (index, query) in report.queries.iter().enumerate() {
        writeln!(
            output,
            "  {}. [{}] {:?}: recall@5={:.3} recall@10={:.3} MRR={:.3}",
            index + 1,
            query.group,
            query.text,
            query.recall_at_5,
            query.recall_at_10,
            query.reciprocal_rank
        )
        .unwrap();
        if !query.missed_expected.is_empty() {
            writeln!(output, "     missed expected UUIDs:").unwrap();
            for uuid in &query.missed_expected {
                writeln!(output, "       {uuid}").unwrap();
            }
            writeln!(output, "     ranked above the misses (top 10 returned):").unwrap();
            for (rank, hit) in query.hits.iter().enumerate() {
                writeln!(
                    output,
                    "       {:>2}. {} score={:.6} {:?}",
                    rank + 1,
                    hit.uuid,
                    hit.score,
                    hit.preview
                )
                .unwrap();
            }
        }
    }
    output
}

fn write_summary(output: &mut String, label: &str, summary: MetricSummary) {
    writeln!(
        output,
        "{label}: queries={} recall@5={:.3} recall@10={:.3} MRR={:.3}",
        summary.queries, summary.recall_at_5, summary.recall_at_10, summary.mrr
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use notes_core::{BlockStyle, db};

    #[tokio::test]
    async fn metrics_are_macro_averaged_and_misses_list_ranked_competitors() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let notes = db::open(directory.path().join("notes.db"))
            .await
            .expect("notes database");
        let page = db::create_page(&notes, "Evaluation".into())
            .await
            .expect("create page");
        let first = db::create_block(
            &notes,
            page.uuid,
            None,
            None,
            BlockStyle::Paragraph,
            "first".into(),
        )
        .await
        .expect("first block");
        let second = db::create_block(
            &notes,
            page.uuid,
            None,
            Some(first.uuid),
            BlockStyle::Paragraph,
            "second".into(),
        )
        .await
        .expect("second block");
        let content = db::get_contents(&notes, vec![first.uuid, second.uuid])
            .await
            .expect("content");
        let hits = content
            .into_iter()
            .enumerate()
            .map(|(index, content)| SearchHit {
                content,
                score: 1.0 - index as f64 / 10.0,
                snippet: None,
            })
            .collect();
        let result = score_query(
            GoldenQuery {
                text: "find second and missing".into(),
                expect: vec![second.uuid, uuid::Uuid::from_u128(99)],
                group: Some("quality".into()),
            },
            hits,
        );

        assert_eq!(result.recall_at_5, 0.5);
        assert_eq!(result.recall_at_10, 0.5);
        assert_eq!(result.reciprocal_rank, 0.5);
        assert_eq!(result.missed_expected, vec![uuid::Uuid::from_u128(99)]);
        let report = EvalReport {
            overall: summarize([&result]),
            groups: BTreeMap::from([("quality".into(), summarize([&result]))]),
            queries: vec![result],
            rerank: false,
        };
        let output = format_report(&report);
        assert!(output.contains("overall: queries=1 recall@5=0.500"));
        assert!(output.contains("ranked above the misses"));
        assert!(output.contains(&first.uuid.to_string()));
    }

    #[test]
    fn shipped_template_keeps_morphology_debt_visible() {
        let set: GoldenSet = toml::from_str(include_str!("../eval-queries.example.toml"))
            .expect("valid example golden set");
        validate_golden_set(&set).expect("valid golden set");
        assert!(
            set.query
                .iter()
                .filter(|query| query.group.as_deref() == Some("morphology"))
                .count()
                >= 3
        );
        assert!(set.query.iter().any(|query| query.text.contains("покупок")));
        for (query_form, indexed_form) in [
            ("покупок", "покупки"),
            ("окон", "окна"),
            ("писем", "письма"),
            ("кресел", "кресла"),
        ] {
            assert_ne!(
                notes_core::stem::stem(query_form),
                notes_core::stem::stem(indexed_form),
                "{query_form}/{indexed_form} must remain a real Snowball divergence"
            );
        }
    }
}
