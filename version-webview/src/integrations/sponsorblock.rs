//! SponsorBlock: segmentos que no son musica (intro, outro, patrocinio...).
//! Port de plugins/sponsorblock de Pear.

use serde::Deserialize;

#[derive(Deserialize)]
struct Submission {
    segment: [f64; 2],
}

/// Ordena y fusiona segmentos solapados (igual que segments.ts de Pear).
pub fn merge(mut segs: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    segs.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    let mut out: Vec<[f64; 2]> = Vec::new();
    for s in segs {
        match out.last_mut() {
            Some(last) if s[0] <= last[1] => last[1] = last[1].max(s[1]),
            _ => out.push(s),
        }
    }
    out
}

pub async fn fetch(
    http: &reqwest::Client,
    api_url: &str,
    categories: &[String],
    video_id: &str,
) -> Vec<[f64; 2]> {
    let cats = serde_json::to_string(categories).unwrap_or_else(|_| "[]".into());
    let url = format!(
        "{}/api/skipSegments?videoID={}&categories={}",
        api_url.trim_end_matches('/'),
        urlencoding::encode(video_id),
        urlencoding::encode(&cats)
    );
    let Ok(resp) = http.get(url).send().await else { return vec![] };
    if !resp.status().is_success() {
        return vec![];
    }
    resp.json::<Vec<Submission>>()
        .await
        .map(|v| merge(v.into_iter().map(|s| s.segment).collect()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn fusiona_segmentos() {
        let m = super::merge(vec![[10.0, 20.0], [0.0, 5.0], [15.0, 30.0], [40.0, 41.0]]);
        assert_eq!(m, vec![[0.0, 5.0], [10.0, 30.0], [40.0, 41.0]]);
    }
}
