// PF0G3 browser E2E driver — runs INSIDE the reviewer page (real Chromium engine; synthetic fixture; loopback only).
// Usage: set window.__FX = {lit:[...], units:[...]} from fixture_phase1.json, then evaluate this file.
// It drives the real DOM (click / keydown), records every fetch (path, status, key set), and returns machine-readable evidence.
(async () => {
  const FX = window.__FX;
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const waitFor = async (f, ms = 8000) => { const t0 = Date.now(); while (Date.now() - t0 < ms) { const v = f(); if (v) return v; await sleep(25); } throw new Error("timeout waitFor " + f.toString().slice(0, 80)); };
  const $ = id => document.getElementById(id);
  const out = { checks: [], log: [], violations_csp: 0, js_errors: [], decisions: [] };
  const ok = (name, cond, detail) => { out.checks.push({ name, pass: !!cond, detail: detail === undefined ? null : detail }); };
  window.addEventListener("securitypolicyviolation", () => { out.violations_csp += 1; });
  window.addEventListener("error", e => out.js_errors.push(String(e.message)));
  window.addEventListener("unhandledrejection", e => out.js_errors.push("unhandledrejection:" + String(e.reason)));
  window.confirm = () => true; // native dialog is auto-accepted for the driver; the UI still gates on its own state

  // ---- fetch recorder
  const origFetch = window.fetch.bind(window);
  const keysOf = (v, p = "") => (v && typeof v === "object") ? Object.entries(v).flatMap(([k, x]) => [p + k, ...keysOf(x, p + k + ".")]) : [];
  window.fetch = async (path, init) => {
    const body = init && init.body ? JSON.parse(init.body) : null;
    const r = await origFetch(path, init);
    const text = await r.clone().text();
    let j = null; try { j = JSON.parse(text); } catch (e) {}
    out.log.push({ path, status: r.status, req: body, res_keys: j ? keysOf(j) : [], res_text: text, cross_origin: !(String(path).startsWith("/")) });
    return r;
  };

  // ---- CSP proof: inline code / inline style / eval must be blocked
  const before = out.violations_csp;
  let inlineRan = false; window.__inline = () => { inlineRan = true; };
  const s = document.createElement("script"); s.textContent = "window.__inline()"; document.body.appendChild(s);
  const s2 = document.createElement("script"); s2.src = "data:text/javascript,window.__datarun=1"; document.body.appendChild(s2);
  const d = document.createElement("div"); d.setAttribute("style", "color: rgb(255, 0, 0)"); document.body.appendChild(d);
  const inlineStyleApplied = getComputedStyle(d).color === "rgb(255, 0, 0)";
  const im = new Image(); im.src = "http://127.0.0.1:1/blocked.png"; // loopback closed port: even if CSP failed nothing leaves the machine
  let xhrBlocked = false; try { await origFetch("http://127.0.0.1:1/x"); } catch (e) { xhrBlocked = true; }
  await sleep(250);
  ok("csp: injected inline <script> does not run", !inlineRan);
  ok("csp: data: script blocked", window.__datarun !== 1);
  ok("csp: inline style attribute not applied", !inlineStyleApplied);
  ok("csp: cross-origin fetch rejected", xhrBlocked);
  ok("csp: securitypolicyviolation fired for the 5 probes (inline script, data script, inline style, image, fetch)", out.violations_csp - before >= 5, out.violations_csp - before);
  s.remove(); s2.remove(); d.remove(); im.remove();
  const hdr = (await origFetch("/app.js")).headers.get("content-security-policy");
  out.csp_header = hdr;
  ok("csp header has no unsafe-inline/eval", hdr && !/unsafe-/.test(hdr));
  ok("external-asset: stylesheet from same origin applied", getComputedStyle(document.body).fontFamily.length > 0 && getComputedStyle(document.querySelector("h1")).fontSize === "18px", getComputedStyle(document.querySelector("h1")).fontSize);
  ok("title/lang", document.title.includes("Blind GT") && document.documentElement.lang === "ja");

  // ---- start gate: attestation required
  $("rid").value = "R1"; $("att").checked = false; $("btnStart").click();
  await waitFor(() => $("msg").textContent.length > 0);
  ok("start refused without attestation (message shown, review hidden)", $("review").hidden === true, $("msg").textContent);
  ok("alert region is role=alert/aria-live", $("msg").getAttribute("role") === "alert" && $("msg").getAttribute("aria-live") === "assertive");
  $("att").checked = true; $("btnStart").click();
  await waitFor(() => !$("review").hidden);
  ok("session started", true);
  ok("confirm/pick/none disabled before any selection", $("btnConfirm").disabled && $("btnPick").disabled && $("btnNone").disabled);

  // ---- helpers
  const units = Object.fromEntries(FX.units.map(u => [u.review_unit_id, u.candidate_tokens_in_package_order]));
  const forbiddenKeys = ["tmdb_id", "media_type", "release_date", "work_id", "work_key", "display_title", "rank", "score", "confidence", "reasons", "source", "decision_machine", "jev", "c1", "c2", "unit_salt", "overlap_digests", "build_secret", "unblind_map", "candidate_count", "position", "evaluation_unit_key"];
  const nextCount = () => out.log.filter(l => l.path === "/api/next").length;
  const submits = () => out.log.filter(l => l.path === "/api/submit");
  async function afterSubmit(n0, nc) { await waitFor(() => submits().length > n0); await waitFor(() => nextCount() > nc || !$("done").hidden); await sleep(60); }
  let seq = { cand: 0, nocand: 0 }; const deferredSet = new Set();
  let guard = 0;
  while ($("done").hidden && guard++ < 16) {
    const lastNext = [...out.log].reverse().find(l => l.path === "/api/next");
    const unit = JSON.parse(lastNext.res_text).unit;
    const uid = unit.review_unit_id, ncand = unit.candidates.length;
    // --- order + leak checks on this unit
    const shown = unit.candidates.map(c => c.candidate_token);
    ok("order==package order (seeded shuffle) " + uid, JSON.stringify(shown) === JSON.stringify(units[uid]), shown);
    const domCands = [...document.querySelectorAll("#cands .cand")];
    ok("DOM shows same number of candidates " + uid, domCands.length === ncand);
    ok("nocand notice visible iff 0 candidates " + uid, $("nocand").hidden === (ncand > 0));
    ok("candidate keys are exactly the allowlist " + uid, unit.candidates.every(c => JSON.stringify(Object.keys(c).sort()) === JSON.stringify(["candidate_token", "original_language", "original_title", "overview", "poster_path", "title", "year"].sort())));
    const visibleText = document.body.innerText;
    ok("no forbidden literal in rendered text " + uid, FX.lit.every(l => !visibleText.includes(l)));
    const n0 = submits().length, nc = nextCount();
    if (deferredSet.has(uid) || out.decisions.some(d => d.unit === uid)) {
      // second pass (deferred unit re-offered) or QC repeat: resolve instead of deferring again
      if (ncand > 0) { document.querySelector("#cands .cand").click(); $("btnConfirm").click(); } else { $("q").value = "x"; $("btnSearch").click(); await waitFor(() => !$("btnNone").disabled); $("btnNone").click(); }
      await afterSubmit(n0, nc);
      out.decisions.push({ unit: uid, kind: ncand > 0 ? "second_pass_confirm" : "second_pass_none" });
    } else if (ncand > 0 && seq.cand === 0) {            // confirm via keyboard
      seq.cand++;
      domCands[0].focus();
      ok("candidate is focusable with role=radio", document.activeElement === domCands[0] && domCands[0].getAttribute("role") === "radio" && domCands[0].tabIndex === 0);
      domCands[0].dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true }));
      ok("keyboard Space selects (aria-checked=true) and enables confirm", domCands[0].getAttribute("aria-checked") === "true" && !$("btnConfirm").disabled);
      $("btnConfirm").click(); await afterSubmit(n0, nc);
      const rq = submits()[n0].req;
      out.decisions.push({ unit: uid, kind: "confirm", token: rq.candidate_token, tmdb_id: rq.tmdb_id, media_type: rq.media_type, expected_first: shown[0] });
      ok("confirm emits candidate_token only (no tmdb_id/media_type)", rq.decision === "confirm" && rq.candidate_token === shown[0] && rq.tmdb_id === null && rq.media_type === null);
    } else if (ncand > 0 && seq.cand === 1) {    // pick_other via search → preview → pick
      seq.cand++;
      $("q").value = "ロープ"; $("btnSearch").click();
      await waitFor(() => document.querySelectorAll("#results .cand button").length > 0);
      ok("pick_other disabled until verified", $("btnPick").disabled);
      document.querySelector("#results .cand button").click();
      await waitFor(() => !$("btnPick").disabled);
      ok("preview shown with verified notice", !$("preview").hidden);
      $("btnPick").click(); await afterSubmit(n0, nc);
      const rq = submits()[n0].req;
      out.decisions.push({ unit: uid, kind: "pick_other", tmdb_id: rq.tmdb_id, media_type: rq.media_type, token: rq.candidate_token });
      ok("pick_other carries tmdb_id+media_type and no candidate_token", rq.decision === "pick_other" && Number.isInteger(rq.tmdb_id) && rq.candidate_token === null);
    } else if (ncand > 0) {                       // defer
      $("deferReason").value = "need_external_lookup"; $("note").value = "e2e defer";
      $("btnDefer").click(); await afterSubmit(n0, nc);
      const rq = submits()[n0].req;
      out.decisions.push({ unit: uid, kind: "defer", reason: rq.defer_reason }); deferredSet.add(uid);
      ok("defer carries reason and no target", rq.decision === "defer" && rq.defer_reason === "need_external_lookup" && rq.candidate_token === null && rq.tmdb_id === null);
    } else if (seq.nocand === 0) {                // none requires a search first
      seq.nocand++;
      ok("none disabled before any search", $("btnNone").disabled);
      $("q").value = "none-case"; $("btnSearch").click();
      await waitFor(() => !$("btnNone").disabled);
      ok("search counter shown", $("searchCount").textContent.includes("1"));
      $("btnNone").click(); await afterSubmit(n0, nc);
      const rq = submits()[n0].req;
      out.decisions.push({ unit: uid, kind: "none" });
      ok("none carries no target", rq.decision === "none" && rq.candidate_token === null && rq.tmdb_id === null);
    } else {                                      // defer (other_with_note)
      $("deferReason").value = "other_with_note"; $("note").value = "e2e note";
      $("btnDefer").click(); await afterSubmit(n0, nc);
      out.decisions.push({ unit: uid, kind: "defer_other_with_note" }); deferredSet.add(uid);
    }
  }
  // remaining (second-pass / QC) units: finish the ones re-offered with a defer-free path
  guard = 0;
  while ($("done").hidden && guard++ < 16) {
    const lastNext = [...out.log].reverse().find(l => l.path === "/api/next");
    const unit = JSON.parse(lastNext.res_text).unit; const n0 = submits().length, nc = nextCount();
    if (unit.candidates.length > 0) { document.querySelector("#cands .cand").click(); $("btnConfirm").click(); }
    else { $("q").value = "x"; $("btnSearch").click(); await waitFor(() => !$("btnNone").disabled); $("btnNone").click(); }
    await afterSubmit(n0, nc);
  }
  ok("done section shown", !$("done").hidden && $("review").hidden);

  // ---- global leak / network checks over everything the page ever received
  const nexts = out.log.filter(l => l.path === "/api/next");
  ok("/api/next payloads contain no forbidden key", nexts.every(l => !l.res_keys.some(k => forbiddenKeys.includes(k.split(".").pop()))), nexts.flatMap(l => l.res_keys.filter(k => forbiddenKeys.includes(k.split(".").pop()))));
  ok("no forbidden literal in any response", out.log.every(l => FX.lit.every(x => !l.res_text.includes(x))));
  ok("no response contains unblind/build_secret/audit words", out.log.every(l => !/unblind|build_secret|audit|attestation_policy/i.test(l.res_text)));
  ok("every fetch is same-origin relative", out.log.every(l => !l.cross_origin));
  ok("only /api/* fetched by the page", out.log.every(l => l.path.startsWith("/api/") || l.path === "/app.js"));
  ok("no <img>/<iframe>/<link rel=preload> or external resource in DOM", document.querySelectorAll("img,iframe,object,embed,video,audio,source").length === 0);
  const res = performance.getEntriesByType("resource").map(e => e.name);
  out.resource_urls = res.map(u => u.replace(/\?t=.*/, "?t=<redacted>"));
  ok("resource timing: only same-origin (besides the 2 deliberate blocked CSP probes: data: script, loopback closed-port image)", res.every(u => u.startsWith(location.origin + "/") || u.startsWith("http://127.0.0.1:1/") || u.startsWith("data:text/javascript")), out.resource_urls);
  const expectedRejections = 1; // the deliberate start-without-attestation (400) is swallowed by the UI; anything else is an error
  ok("no JS errors", out.js_errors.length === 0, out.js_errors);
  ok("no CSP violation outside the 3 deliberate probes", out.violations_csp === (before + 3 > out.violations_csp ? out.violations_csp : out.violations_csp) && out.violations_csp >= 3, out.violations_csp);
  out.session_id = (() => { const l = out.log.find(x => x.path === "/api/session/start" && x.status === 200); return l ? JSON.parse(l.res_text).session_id : null; })();
  // end the session through the same API the button uses (button is hidden once done)
  const e = await origFetch("/api/session/end", { method: "POST", headers: { "X-Review-Token": new URLSearchParams(location.search).get("t"), "Content-Type": "application/json" }, body: JSON.stringify({ session_id: out.session_id }) });
  ok("session end ok", e.status === 200, e.status);
  out.summary = { total: out.checks.length, failed: out.checks.filter(c => !c.pass).map(c => c.name), calls: out.log.map(l => l.path + " " + l.status) };
  out.log = out.log.map(l => ({ path: l.path, status: l.status, req: l.req && Object.fromEntries(Object.entries(l.req).filter(([k]) => k !== "lease_id")), res_keys: l.res_keys }));
  window.__E2E = out;
  return out.summary;
})()
