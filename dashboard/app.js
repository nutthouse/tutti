// tutti factory floor — vanilla JS, no build step
"use strict";

// The canonical pipeline stage order
var STAGE_ORDER = ["planner", "implementer", "tester", "reviewer", "docs-release"];

// Map agent names to their pipeline stage (loose matching)
function stageFor(agent) {
  var a = agent.toLowerCase();
  for (var i = 0; i < STAGE_ORDER.length; i++) {
    var s = STAGE_ORDER[i];
    if (a.indexOf(s.replace("-", "")) !== -1 || a.indexOf(s) !== -1) return s;
  }
  return null;
}

// ── State ──
var appState = {
  agents: {},      // composite key (workspace:agent) -> latest health record
  events: [],      // recent events (newest first), capped at 50
  eventCount: 0,
  selectedAgent: null, // composite key of currently selected agent
};

// Build a composite key for workspace-scoped agent storage
function agentKey(workspace, agent) {
  return (workspace || "_") + ":" + agent;
}

// ── DOM refs ──
var $pipeline    = document.getElementById("pipeline");
var $eventList   = document.getElementById("event-list");
var $wsName      = document.getElementById("workspace-name");
var $hudActive   = document.getElementById("hud-active");
var $hudBlock    = document.getElementById("hud-blocked");
var $hudBottle   = document.getElementById("hud-bottleneck");
var $connDot     = document.getElementById("conn-status");
var $drawer      = document.getElementById("detail-drawer");
var $detailName  = document.getElementById("detail-name");
var $detailMeta  = document.getElementById("detail-meta");
var $detailEvts  = document.getElementById("detail-events");
var $detailClose = document.getElementById("detail-close");

// ── Classify agent state into a CSS class ──
function stateClass(agent) {
  if (!agent.running) return "stopped";
  if (agent.auth_state === "expired" || agent.auth_state === "failed")
    return "auth-fail";
  if (agent.activity_state === "blocked" || agent.activity_state === "rate_limited")
    return "blocked";
  if (agent.activity_state === "working" || agent.activity_state === "active")
    return "working";
  return "idle";
}

function stateLabel(agent) {
  if (!agent.running) return "stopped";
  if (agent.auth_state === "expired" || agent.auth_state === "failed")
    return "auth fail";
  if (agent.activity_state === "blocked") return "blocked";
  if (agent.activity_state === "rate_limited") return "rate limited";
  if (agent.activity_state === "working" || agent.activity_state === "active")
    return "working";
  return "idle";
}

// ── Safe DOM element creation ──
function el(tag, className, textContent) {
  var node = document.createElement(tag);
  if (className) node.className = className;
  if (textContent) node.textContent = textContent;
  return node;
}

// ── Time formatting ──
function timeAgo(ts) {
  if (!ts) return "—";
  var diff = (Date.now() - new Date(ts).getTime()) / 1000;
  if (diff < 0) return "just now";
  if (diff < 60) return Math.floor(diff) + "s ago";
  if (diff < 3600) return Math.floor(diff / 60) + "m ago";
  return Math.floor(diff / 3600) + "h ago";
}

// ── Bottleneck detection ──
// Returns the agent that has been working longest, or null if none working
function findBottleneck() {
  var longest = null;
  var longestDuration = 0;
  var now = Date.now();
  var names = Object.keys(appState.agents);
  for (var i = 0; i < names.length; i++) {
    var agent = appState.agents[names[i]];
    if (stateClass(agent) !== "working") continue;
    var ts = agent.last_output_change_at;
    if (!ts) continue;
    var duration = now - new Date(ts).getTime();
    if (duration > longestDuration) {
      longestDuration = duration;
      longest = agent;
    }
  }
  return longest;
}

// ── Render the pipeline ──
function renderPipeline() {
  while ($pipeline.firstChild) $pipeline.removeChild($pipeline.firstChild);

  // Build a map of stage -> agents
  var stageAgents = {};
  var i;
  for (i = 0; i < STAGE_ORDER.length; i++) stageAgents[STAGE_ORDER[i]] = [];
  var names = Object.keys(appState.agents);
  for (i = 0; i < names.length; i++) {
    var agent = appState.agents[names[i]];
    var s = stageFor(agent.agent);
    if (s && stageAgents[s]) {
      stageAgents[s].push(agent);
    }
  }

  for (i = 0; i < STAGE_ORDER.length; i++) {
    var stage = STAGE_ORDER[i];

    // Flow connector between stages
    if (i > 0) {
      var conn = el("div", "flow-connector");
      var prevAgents = stageAgents[STAGE_ORDER[i - 1]] || [];
      var prevActive = false;
      for (var p = 0; p < prevAgents.length; p++) {
        if (stateClass(prevAgents[p]) === "working") { prevActive = true; break; }
      }
      if (prevActive) conn.classList.add("active");
      $pipeline.appendChild(conn);
    }

    var agents = stageAgents[stage];
    var card = el("div", "stage");

    if (agents.length === 0) {
      card.classList.add("empty");
      card.appendChild(el("div", "stage-name", stage));
      card.appendChild(el("span", "state-chip", "\u2014"));
    } else {
      var primary = agents[0];
      var key = agentKey(primary.workspace, primary.agent);
      card.classList.add(stateClass(primary));
      card.setAttribute("data-agent-key", key);
      if (appState.selectedAgent === key) card.classList.add("selected");
      card.appendChild(el("div", "stage-name", stage));
      card.appendChild(el("span", "state-chip", stateLabel(primary)));
      card.appendChild(el("div", "agent-runtime", primary.runtime || "\u2014"));

      // Click handler for detail drawer
      card.addEventListener("click", (function(k) {
        return function() { selectAgent(k); };
      })(key));
    }

    $pipeline.appendChild(card);
  }

  // Update HUD
  var active = 0, blocked = 0;
  var allNames = Object.keys(appState.agents);
  for (i = 0; i < allNames.length; i++) {
    var cls = stateClass(appState.agents[allNames[i]]);
    if (cls === "working") active++;
    if (cls === "blocked" || cls === "auth-fail") blocked++;
  }
  $hudActive.textContent = active + " active";
  $hudBlock.textContent = blocked + " blocked";
  $hudBlock.className = "hud-item" + (blocked > 0 ? " alert" : "");

  // Bottleneck indicator
  var bottleneck = findBottleneck();
  if (bottleneck) {
    var dur = timeAgo(bottleneck.last_output_change_at);
    $hudBottle.textContent = bottleneck.agent + " " + dur;
    $hudBottle.className = "hud-item warn";
    $hudBottle.title = "longest-working agent (potential bottleneck)";
  } else {
    $hudBottle.textContent = "";
    $hudBottle.className = "hud-item";
  }
}

// ── Detail drawer ──
function selectAgent(key) {
  if (appState.selectedAgent === key) {
    closeDrawer();
    return;
  }
  appState.selectedAgent = key;
  var agent = appState.agents[key];
  if (!agent) { closeDrawer(); return; }

  $detailName.textContent = agent.agent;

  // Meta info
  $detailMeta.innerHTML = "";
  var meta = [
    ["state", stateLabel(agent)],
    ["runtime", agent.runtime || "—"],
    ["session", agent.session_name || "—"],
    ["last change", timeAgo(agent.last_output_change_at)],
    ["last probe", timeAgo(agent.last_probe_at)],
  ];
  if (agent.reason) meta.push(["reason", agent.reason]);
  for (var i = 0; i < meta.length; i++) {
    var s = el("span", null, null);
    s.innerHTML = "<span>" + meta[i][0] + ":</span> " + meta[i][1];
    $detailMeta.appendChild(s);
  }

  // Filter events for this agent
  while ($detailEvts.firstChild) $detailEvts.removeChild($detailEvts.firstChild);
  var agentName = agent.agent;
  var count = 0;
  for (var j = 0; j < appState.events.length && count < 15; j++) {
    var evt = appState.events[j];
    if (evt.agent !== agentName) continue;
    var li = document.createElement("li");
    li.appendChild(el("span", "evt-type", evt.event));
    li.appendChild(document.createTextNode(" "));
    li.appendChild(el("span", "evt-time", timeAgo(evt.timestamp)));
    $detailEvts.appendChild(li);
    count++;
  }
  if (count === 0) {
    $detailEvts.appendChild(el("li", null, "no recent events"));
  }

  $drawer.classList.add("open");
  renderPipeline(); // re-render to show selected state
}

function closeDrawer() {
  appState.selectedAgent = null;
  $drawer.classList.remove("open");
  renderPipeline();
}

$detailClose.addEventListener("click", closeDrawer);

// ── Render the event timeline ──
function renderTimeline() {
  while ($eventList.firstChild) $eventList.removeChild($eventList.firstChild);

  var limit = Math.min(appState.events.length, 30);
  for (var i = 0; i < limit; i++) {
    var evt = appState.events[i];
    var li = document.createElement("li");

    var typeSpan = el("span", "evt-type", evt.event);
    li.appendChild(typeSpan);
    li.appendChild(document.createTextNode(" "));

    if (evt.agent) {
      var agentSpan = el("span", "evt-agent", evt.agent);
      li.appendChild(agentSpan);
      li.appendChild(document.createTextNode(" "));
    }

    var timeSpan = el("span", "evt-time", timeAgo(evt.timestamp));
    li.appendChild(timeSpan);

    $eventList.appendChild(li);
  }
}

// ── Debounce render to prevent flicker ──
var renderTimer = null;
function scheduleRender() {
  if (renderTimer) return;
  renderTimer = setTimeout(function() {
    renderTimer = null;
    renderPipeline();
    renderTimeline();
  }, 300);
}

// ── Fetch initial health snapshot ──
function fetchHealth() {
  return fetch("/v1/health").then(function(res) {
    return res.json();
  }).then(function(json) {
    var records = json.data || [];
    var fresh = {};
    for (var i = 0; i < records.length; i++) {
      var r = records[i];
      var key = agentKey(r.workspace, r.agent);
      fresh[key] = r;
      if (r.workspace) $wsName.textContent = r.workspace;
    }
    appState.agents = fresh;
    renderPipeline();
  }).catch(function(e) {
    console.warn("health fetch failed:", e);
  });
}

// ── SSE connection ──
function connectSSE() {
  var es = new EventSource("/v1/events/stream");

  es.onopen = function() {
    $connDot.className = "conn-dot connected";
  };

  es.onerror = function() {
    $connDot.className = "conn-dot error";
  };

  var handler = function(e) {
    try {
      var data = JSON.parse(e.data);
      appState.eventCount++;
      appState.events.unshift(data);
      if (appState.events.length > 50) appState.events.length = 50;

      if (data.workspace) $wsName.textContent = data.workspace;

      // If the event carries agent data, update agent state
      if (data.agent && data.data) {
        var key = agentKey(data.workspace, data.agent);
        var existing = appState.agents[key] || {};
        var merged = {};
        var k;
        for (k in existing) merged[k] = existing[k];
        for (k in data.data) merged[k] = data.data[k];
        merged.agent = data.agent;
        if (data.workspace) merged.workspace = data.workspace;
        appState.agents[key] = merged;
      }

      scheduleRender();
    } catch (_) { /* ignore parse errors */ }
  };

  // Event types actually emitted by the server (state/mod.rs transition_events)
  var eventTypes = [
    "agent.started", "agent.stopped",
    "agent.working", "agent.idle",
    "agent.auth_failed", "agent.auth_recovered",
    "agent.rate_limited", "agent.provider_down", "agent.provider_recovered"
    // Phase 1b will add: workflow.step, workflow.stage_transition, run.completed
  ];
  for (var i = 0; i < eventTypes.length; i++) {
    es.addEventListener(eventTypes[i], handler);
  }

  // Also listen for unnamed "message" events
  es.onmessage = handler;
}

// ── Boot ──
fetchHealth().then(function() {
  connectSSE();
  // Re-fetch health periodically to stay in sync
  setInterval(fetchHealth, 15000);
});
