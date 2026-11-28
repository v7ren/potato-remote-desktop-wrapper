const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

// tiny DOM builder: h("div.cls", {attr}, ...children). Text goes in as text nodes, never HTML.
function h(tag, attrs, ...kids) {
  const [t, ...cls] = tag.split(".");
  const el = document.createElement(t);
  if (cls.length) el.className = cls.join(" ");
  for (const [k, v] of Object.entries(attrs || {})) {
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (v === true) el.setAttribute(k, "");
    else if (v !== false && v != null) el.setAttribute(k, v);
  }
  for (const c of kids.flat()) if (c != null && c !== false) el.append(c);
  return el;
}
const icon = (id) => { const s = document.createElementNS("http://www.w3.org/2000/svg", "svg"); s.innerHTML = `<use href="#${id}"/>`; s.setAttribute("aria-hidden", "true"); return s; };
const pad2 = (n) => String(n).padStart(2, "0");

function toast(msg) {
  const t = h("div.toast", {}, h("span.led.on"), String(msg));
  $("toasts").append(t);
  setTimeout(() => t.remove(), 4500);
}
function banner(msg) { $("bannerText").textContent = msg; $("banner").hidden = !msg; }
$("bannerX").onclick = () => banner("");

let S = null;                 // last state
let hostUi = new Map();       // ip -> {screen, busy, error}: survives re-renders
let hostSig = "";
let signing = false;          // waiting for the browser sign-in
let pollTimer = null;

// ---------- pages ----------
function go(page) {
  for (const b of document.querySelectorAll(".nav"))
    b.dataset.page === page ? b.setAttribute("aria-current", "page") : b.removeAttribute("aria-current");
  for (const s of document.querySelectorAll(".page")) s.hidden = s.id !== "page-" + page;
}
for (const b of document.querySelectorAll(".nav")) b.onclick = () => go(b.dataset.page);

// ---------- state ----------
async function refresh() {
  try { S = await invoke("state"); } catch (e) { toast("Couldn't read state: " + e); return; }
  if (S.error) banner(S.error);
  const needs = S.missing && S.missing.length;
  const signedIn = S.signed_in && !needs;
  if (signedIn) signing = false;
  $("welcome").hidden = signedIn;
  $("app").hidden = !signedIn;
  if (!signedIn) {
    $("welcomeText").textContent = needs ? `Install ${S.missing.join(" and ")} first, then reopen Solinear.`
      : "Sign in once, then use the same account on every device.";
    $("signinBtn").hidden = !!needs;
    $("signinBtn").disabled = signing;
    $("signinHint").hidden = !signing;
    return;
  }
  const acct = S.account || "Signed in";
  $("acct").textContent = acct;
  $("acct").title = acct;
  $("avatar").textContent = acct.replace(/[^a-z0-9]/gi, "").slice(0, 2).toUpperCase() || "··";
  $("mod").textContent = S.mac ? "Ctrl+Option+Shift" : "Ctrl+Alt+Shift";
  renderThisPc();
  loadHosts();
}

function renderThisPc() {
  const kind = S.mac ? "Mac" : "PC";
  $("navThis").textContent = `This ${kind}`;
  $("thisTitle").textContent = `THIS ${kind.toUpperCase()}`;
  $("hostTitle").textContent = `Let my other devices stream this ${kind}`;
  $("deviceName").textContent = S.device_name || "";
  const sw = $("hostSwitch");
  sw.setAttribute("aria-checked", String(!!S.is_host));
  sw.disabled = !S.can_host;
  const on = !!S.is_host;
  $("hostStatus").textContent = !S.can_host ? "Not available" : on ? "Ready" : "Off";
  $("hostLed").className = "led" + (on ? " red pulse" : "");
  $("navLed").className = "led" + (on ? " red pulse" : "");
  const note = $("hostNote");
  note.hidden = false;
  if (!S.can_host) note.textContent = S.mac
    ? "To stream this Mac, install Sunshine (brew tap LizardByte/homebrew && brew install sunshine) and reopen Solinear."
    : "Streaming from this PC isn't available yet.";
  else if (on && !S.mac) note.textContent = "Solinear runs as admin while hosting, so your input reaches games that run as admin.";
  else if (!on && !S.mac) note.textContent = "Turning this on asks Windows for admin: games that run as admin only take input from an admin app.";
  else note.hidden = true;
  $("macPerms").hidden = !(S.mac && on);
}

$("hostSwitch").onclick = async (e) => {
  const sw = e.currentTarget, want = sw.getAttribute("aria-checked") !== "true";
  sw.disabled = true; sw.setAttribute("aria-checked", String(want));
  try { S = await invoke("set_hosting", { enabled: want }); }
  catch (err) { banner("Setup failed: " + err); }
  refresh();
};

// ---------- sign in / switch ----------
// the system opens the link in the default browser's last profile, which may be the wrong one
const showLink = (r) => ($("signinUrl").value = (r && r.url) || "");
$("copyUrl").onclick = async () => {
  try { await navigator.clipboard.writeText($("signinUrl").value); $("copyUrl").textContent = "Copied"; }
  catch { $("signinUrl").select(); }
};
function waitForSignIn() {
  signing = true;
  refresh();
  clearInterval(pollTimer);
  pollTimer = setInterval(async () => { await refresh(); if (!signing) clearInterval(pollTimer); }, 1500);
}
$("signinBtn").onclick = async () => {
  $("signinBtn").disabled = true;
  try { showLink(await invoke("sign_in")); waitForSignIn(); }
  catch (e) { toast("Sign-in failed: " + e); $("signinBtn").disabled = false; }
};
$("switchBtn").onclick = async () => {
  try { showLink(await invoke("switch_account")); hostSig = ""; waitForSignIn(); }
  catch (e) { toast("Couldn't switch account: " + e); }
};

// ---------- devices ----------
async function loadHosts(force) {
  let list;
  try { list = await invoke("hosts"); } catch (e) { return; }
  const sig = JSON.stringify(list);
  if (sig === hostSig && !force) return;   // don't rebuild (and steal focus) when nothing changed
  hostSig = sig;
  $("navCount").textContent = pad2(list.length);
  const box = $("hosts");
  box.replaceChildren();
  if (!list.length) {
    box.append(h("div.card.empty", {}, h("div.os", {}, icon("i-off")),
      h("div.dot", {}, "NO DEVICES FOUND"),
      h("p", {}, "Install Solinear on your other PC, sign in with the same account and turn on hosting under This PC.")));
    return;
  }
  for (const host of list) box.append(hostCard(host));
}
$("scanBtn").onclick = async (e) => {
  const b = e.currentTarget; b.disabled = true;
  await loadHosts(true);
  b.disabled = false;
};

function hostCard(host) {
  const ui = hostUi.get(host.ip) || {};
  hostUi.set(host.ip, ui);
  const screens = host.screens || [];
  if (!ui.screen || !screens.some(s => s.id === ui.screen)) ui.screen = (screens.find(s => s.primary) || screens[0] || {}).id ?? null;

  const badge = host.online ? `${host.direct ? "Direct" : "Relayed"}${host.ms != null ? ` · ${host.ms}ms` : ""}` : "Offline";
  const pick = screens.length > 1
    ? h("div.seg.sm", { role: "group", "aria-label": "Screen" }, screens.map((s, i) => {
        const b = h("button", { "aria-pressed": String(s.id === ui.screen), title: `${s.w}×${s.h}${s.primary ? " · main" : ""}` }, `Screen ${i + 1}`);
        b.onclick = () => { ui.screen = s.id; for (const x of b.parentNode.children) x.setAttribute("aria-pressed", String(x === b)); };
        return b;
      }))
    : h("span.single", {}, "Single display");
  const err = h("div.err", { role: "alert", hidden: !ui.error }, h("span.led.red"), h("span", {}, ui.error || ""));
  const btn = h("button.btn", {});
  const paint = () => {
    btn.disabled = !host.online || !!ui.busy;
    btn.replaceChildren(...(ui.busy ? [h("span.spin"), "CONNECTING"] : [icon("i-play"), "PLAY"]));
    err.hidden = !ui.error; err.lastChild.textContent = ui.error || "";
  };
  btn.onclick = async () => {
    ui.busy = true; ui.error = null; paint();
    try {
      const r = await invoke("play", { ip: host.ip, name: host.name, screen: screens.length > 1 ? ui.screen : null });
      if (!r.ok) ui.error = r.error || "Couldn't start the stream.";
    } catch (e) { ui.error = "Couldn't start the stream: " + e; }
    ui.busy = false; paint();
  };
  paint();
  return h("div.card.host" + (host.online ? "" : ".off"), {},
    h("div.top", {},
      h("div.os", {}, icon(host.os === "mac" ? "i-mac" : "i-win")),
      h("div.grow", {},
        h("div.nm", {}, h("span.led" + (host.online ? ".on" : ""), { title: host.online ? "Online" : "Offline" }), h("b", { title: host.name }, host.name),
          host.this_device && h("span.tag", { title: "The PC you're using now. Streaming it shows a screen inside a screen: handy only for testing." }, "THIS DEVICE")),
        h("span.badge", {}, badge))),
    h("div.bottom", {}, pick, btn),
    err);
}

// ---------- settings ----------
let settings = {};
const save = async () => {
  try { settings = await invoke("save_settings", settings); } catch (e) { toast("Couldn't save settings: " + e); }
};
function paintSettings() {
  for (const seg of document.querySelectorAll("#page-settings .seg"))
    for (const b of seg.children) b.setAttribute("aria-pressed", String(settings[seg.dataset.key] === b.dataset.v));
  for (const id of ["reverse_scroll", "match_keys"]) $(id).setAttribute("aria-checked", String(!!settings[id]));
}
for (const seg of document.querySelectorAll("#page-settings .seg"))
  for (const b of seg.children) b.onclick = () => { settings[seg.dataset.key] = b.dataset.v; paintSettings(); save(); };
for (const id of ["reverse_scroll", "match_keys"]) $(id).onclick = () => { settings[id] = !settings[id]; paintSettings(); save(); };
async function loadSettings() {
  try { settings = await invoke("settings"); paintSettings(); } catch (e) { toast("Couldn't load settings: " + e); }
}

// ---------- boot ----------
listen("toast", (e) => toast(e.payload));
listen("state-changed", () => { hostSig = ""; refresh(); });
window.__TAURI__.app?.getVersion?.().then(v => ($("version").textContent = "v" + v)).catch(() => {});
go("devices");
loadSettings();
refresh();
setInterval(() => { if (!signing) refresh(); }, 15000);
