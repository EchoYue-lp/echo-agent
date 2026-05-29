//! Interactive eval review — self-contained HTML with embedded JS.
//!
//! Generates an HTML page that can be opened directly in a browser.
//! No server needed. Supports filtering, sorting, and feedback export.

use crate::eval::EvalReport;

/// Feedback entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewFeedback {
    pub case_id: String,
    pub rating: u8,
    pub comment: String,
    pub timestamp: String,
}

/// Generate a self-contained interactive review HTML page.
pub fn generate_review_html(report: &EvalReport, title: &str) -> String {
    let json_data = serde_json::to_string(report).unwrap_or_default();
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="UTF-8"><title>{title}</title>
<style>
 body {{ font-family: system-ui; max-width: 960px; margin: 2rem auto; padding: 0 1rem; }}
 .card {{ background: #f8fafc; border: 1px solid #e2e8f0; border-radius: 8px; padding: 1rem; margin: 0.5rem; cursor: pointer; }}
 .card:hover {{ background: #e2e8f0; }}
 .pass {{ border-left: 4px solid #22c55e; }}
 .fail {{ border-left: 4px solid #ef4444; }}
 .feedback {{ margin-top: 0.5rem; }}
 .feedback textarea {{ width: 100%; height: 60px; }}
 .hidden {{ display: none; }}
 .filter-bar {{ margin: 1rem 0; display: flex; gap: 0.5rem; }}
 .filter-bar button {{ padding: 0.25rem 0.75rem; border: 1px solid #cbd5e1; border-radius: 4px; background: #fff; cursor: pointer; }}
 .filter-bar button.active {{ background: #3b82f6; color: #fff; }}
</style></head><body>
<h1>{title}</h1>
<div class="filter-bar">
 <button class="active" onclick="filter('all')">All</button>
 <button onclick="filter('pass')">Passed</button>
 <button onclick="filter('fail')">Failed</button>
 <button onclick="exportFeedback()">Export Feedback</button>
</div>
<div id="cases"></div>
<script>
const REPORT = {json_data};
const feedback = {{}};
function render(filter) {{
 document.getElementById('cases').innerHTML = REPORT.results.map(r => {{
  if (filter === 'pass' && !r.success) return '';
  if (filter === 'fail' && r.success) return '';
  return `<div class="card ${{r.success?'pass':'fail'}}" onclick="toggleFeedback('${{r.case_id}}')">
   <strong>${{r.success?'PASS':'FAIL'}}</strong> ${{r.case_id}} — score: ${{r.score.toFixed(2)}}
   <div class="feedback hidden" id="fb-${{r.case_id}}">
    <textarea id="txt-${{r.case_id}}" placeholder="Leave feedback..."></textarea>
    <button onclick="saveFeedback('${{r.case_id}}')">Save</button>
   </div>
  </div>`;
 }}).join('');
}}
function toggleFeedback(id) {{ document.getElementById('fb-'+id).classList.toggle('hidden'); }}
function saveFeedback(id) {{
 feedback[id] = document.getElementById('txt-'+id).value;
 alert('Feedback saved for '+id);
}}
function filter(f) {{
 document.querySelectorAll('.filter-bar button').forEach(b=>b.classList.remove('active'));
 event.target.classList.add('active');
 render(f);
}}
function exportFeedback() {{
 const blob = new Blob([JSON.stringify(feedback,null,2)], {{type:'application/json'}});
 const a = document.createElement('a'); a.href = URL.createObjectURL(blob);
 a.download = 'feedback.json'; a.click();
}}
render('all');
</script></body></html>"#,
        title = title,
        json_data = json_data
    )
}
