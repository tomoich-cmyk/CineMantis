"use strict";
const token = new URLSearchParams(location.search).get("t") || "";
let session = null, unit = null, picked = null, verified = null, searches = 0, prev = "";
const $ = id => document.getElementById(id);
function text(parent, tag, s, cls) { const e = document.createElement(tag); e.textContent = s; if (cls) e.className = cls; parent.appendChild(e); return e; }
async function api(path, body) {
  const r = await fetch(path, { method: body ? "POST" : "GET", headers: { "X-Review-Token": token, "Content-Type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
  const j = await r.json();
  if (!r.ok) { $("msg").textContent = j.message || "エラー"; throw new Error(j.reason_code); }
  $("msg").textContent = "";
  return j;
}
function uid() { return (crypto.randomUUID ? crypto.randomUUID() : String(Date.now()) + Math.random()); }
function refresh() {
  $("btnConfirm").disabled = picked === null;
  $("btnPick").disabled = verified === null;
  $("btnNone").disabled = searches < 1;
  $("searchCount").textContent = "検索 " + searches + " 回";
}
function show(u, p) {
  unit = u; picked = null; verified = null; searches = 0; prev = u.prev_event_sha;
  $("prog").textContent = "完了 " + p.done + " / 残り " + p.remaining + " / 保留 " + p.deferred;
  const f = $("files"); f.replaceChildren();
  u.files.forEach(x => { const d = text(f, "div", "", "box"); text(d, "pre", "パート " + x.part_no); text(d, "pre", x.original_file_name); text(d, "pre", x.original_rel_path, "mut"); text(d, "pre", x.container_tags_json, "mut"); text(d, "pre", x.source_media_kind, "mut"); });
  const c = $("cands"); c.replaceChildren(); $("nocand").hidden = u.candidates.length > 0;
  u.candidates.forEach(x => {
    const d = text(c, "div", "", "cand"); d.tabIndex = 0; d.setAttribute("role", "radio"); d.setAttribute("aria-checked", "false");
    text(d, "strong", x.title + (x.year ? "（" + x.year + "）" : ""));
    if (x.original_title) text(d, "div", x.original_title, "mut");
    if (x.overview) text(d, "div", x.overview);
    const pick = () => { picked = x.candidate_token; [...c.children].forEach(e => { e.classList.remove("sel"); e.setAttribute("aria-checked", "false"); }); d.classList.add("sel"); d.setAttribute("aria-checked", "true"); refresh(); };
    d.onclick = pick; d.onkeydown = e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); pick(); } };
  });
  $("results").replaceChildren(); $("preview").hidden = true; $("note").value = ""; $("q").value = "";
  refresh();
}
async function next() {
  const n = await api("/api/next", { session_id: session });
  if (n.done) { $("review").hidden = true; $("done").hidden = false; return; }
  show(n.unit, n.progress);
}
async function submit(extra) {
  const body = Object.assign({ unit: unit.review_unit_id, lease_id: unit.lease_id, submission_id: uid(), prev_event_sha: prev, decision: "", candidate_token: null, tmdb_id: null, media_type: null, defer_reason: null, note: $("note").value || null, correction_reason: null }, extra);
  await api("/api/submit", body); await next();
}
$("btnStart").onclick = async () => {
  const s = await api("/api/session/start", { reviewer_id: $("rid").value, attestation: $("att").checked });
  session = s.session_id; $("start").hidden = true; $("review").hidden = false; await next();
};
$("btnEnd").onclick = async () => { await api("/api/session/end", { session_id: session }); location.reload(); };
$("btnConfirm").onclick = () => submit({ decision: "confirm", candidate_token: picked });
$("btnNone").onclick = () => { if (confirm("TMDB に該当が無いことを確認しました。「該当なし」で確定しますか？")) submit({ decision: "none" }); };
$("btnDefer").onclick = () => submit({ decision: "defer", defer_reason: $("deferReason").value });
$("btnPick").onclick = () => { if (verified && confirm("この作品で確定しますか？")) submit({ decision: "pick_other", tmdb_id: verified.tmdb_id, media_type: verified.media_type }); };
async function preview(id, mt) {
  verified = null; refresh();
  const p = await api("/api/preview", { unit: unit.review_unit_id, lease_id: unit.lease_id, tmdb_id: id, media_type: mt });
  verified = p; const b = $("preview"); b.replaceChildren(); b.hidden = false;
  text(b, "strong", p.title + (p.year ? "（" + p.year + "）" : "")); if (p.original_title) text(b, "div", p.original_title, "mut"); if (p.overview) text(b, "div", p.overview);
  text(b, "div", "TMDB で存在を確認しました。「候補外を指定」で確定できます。", "mut"); refresh();
}
$("btnPreview").onclick = () => { const id = Number($("pid").value); if (!Number.isSafeInteger(id) || id < 1) { $("msg").textContent = "ID は 1 以上の整数です。"; return; } preview(id, $("pt").value); };
$("btnSearch").onclick = async () => {
  const mt = $("qt").value;
  const r = await api("/api/search", { unit: unit.review_unit_id, lease_id: unit.lease_id, query: $("q").value, media_type: mt });
  prev = r.prev_event_sha; searches += 1; refresh();
  const box = $("results"); box.replaceChildren();
  r.results.forEach(x => {
    const d = text(box, "div", "", "cand");
    text(d, "strong", x.title + (x.year ? "（" + x.year + "）" : "")); if (x.already_shown) text(d, "span", "表示済み候補です", "badge");
    if (x.overview) text(d, "div", x.overview);
    if (!x.already_shown) { const b = text(d, "button", "この作品を確認"); b.onclick = () => { $("pid").value = x.tmdb_id; $("pt").value = x.media_type; preview(x.tmdb_id, x.media_type); }; }
  });
};
// api() はエラーを #msg に出してから reason_code で reject する。ハンドラは catch しないので、その既知の reject だけ握りつぶす（他のエラーは通常どおり console に出る）
window.addEventListener("unhandledrejection", e => { if (e.reason instanceof Error && /^[a-z_]+$/.test(e.reason.message)) e.preventDefault(); });
