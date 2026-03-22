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
  selectedRun: null,   // correlation_id of currently selected run
  runs: {},        // correlation_id -> run state { stage, status, steps, workflow_name }
};

// ── Run tracking ──
// Maps a step's agent to a pipeline stage so we can position the dot
function runStageFromEvent(evt) {
  if (!evt || !evt.data) return null;
  var agent = evt.agent || (evt.data && evt.data.agent);
  if (agent) return stageFor(agent);
  return null;
}

// Format milliseconds as human-readable duration
function formatDuration(ms) {
  if (!ms && ms !== 0) return "—";
  if (ms < 1000) return ms + "ms";
  var s = Math.floor(ms / 1000);
  if (s < 60) return s + "s";
  var m = Math.floor(s / 60);
  s = s % 60;
  return m + "m " + s + "s";
}

// Process a workflow event and update run tracking state
function processWorkflowEvent(evt) {
  var id = evt.correlation_id;
  if (!id) return;

  if (evt.event === "workflow.started") {
    appState.runs[id] = {
      id: id,
      status: "running",
      stage: null,
      stepIndex: 0,
      totalSteps: (evt.data && evt.data.total_steps) || 0,
      workflowName: (evt.data && evt.data.workflow_name) || "",
      startedAt: evt.timestamp,
      steps: []  // step timeline entries
    };
    return;
  }

  var run = appState.runs[id];
  if (!run) return;

  if (evt.event === "workflow.step.started") {
    run.stage = runStageFromEvent(evt);
    run.stepIndex = (evt.data && evt.data.step_index) || run.stepIndex;
    run.totalSteps = (evt.data && evt.data.total_steps) || run.totalSteps;
    run.status = "running";
    // Record step start in timeline
    var idx = evt.data && evt.data.step_index;
    if (idx) {
      run.steps[idx] = {
        index: idx,
        type: (evt.data && evt.data.step_type) || "unknown",
        agent: evt.agent || null,
        stage: run.stage,
        status: "running",
        startedAt: evt.timestamp,
        durationMs: null,
        message: null,
        artifactName: (evt.data && evt.data.artifact_name) || null
      };
    }
  } else if (evt.event === "workflow.step.completed") {
    var cidx = evt.data && evt.data.step_index;
    if (cidx && run.steps[cidx]) {
      run.steps[cidx].status = "completed";
      run.steps[cidx].durationMs = (evt.data && evt.data.duration_ms != null) ? evt.data.duration_ms : null;
      run.steps[cidx].message = (evt.data && evt.data.message) || null;
    }
  } else if (evt.event === "workflow.step.failed") {
    run.stage = runStageFromEvent(evt);
    run.status = "failed";
    var fidx = evt.data && evt.data.step_index;
    if (fidx && run.steps[fidx]) {
      run.steps[fidx].status = "failed";
      run.steps[fidx].durationMs = (evt.data && evt.data.duration_ms) || null;
      run.steps[fidx].message = (evt.data && evt.data.message) || null;
    }
  } else if (evt.event === "workflow.completed") {
    run.status = "completed";
    run.finishedAt = evt.timestamp;
    // Auto-remove completed runs after 8 seconds so the dot exits
    setTimeout(function() {
      delete appState.runs[id];
      scheduleRender();
    }, 8000);
  } else if (evt.event === "workflow.failed") {
    run.status = "failed";
    run.finishedAt = evt.timestamp;
  }
}

// Get active (non-terminal) runs at a given stage
function runsAtStage(stage) {
  var result = [];
  var ids = Object.keys(appState.runs);
  for (var i = 0; i < ids.length; i++) {
    var run = appState.runs[ids[i]];
    if (run.stage === stage && run.status !== "completed" && run.status !== "failed") result.push(run);
  }
  return result;
}

// Find artifact name flowing between two pipeline stages (from active runs)
function artifactBetweenStages(fromStage, toStage) {
  var ids = Object.keys(appState.runs);
  for (var i = 0; i < ids.length; i++) {
    var run = appState.runs[ids[i]];
    if (!run.steps || run.status !== "running") continue;
    for (var j = 0; j < run.steps.length; j++) {
      var step = run.steps[j];
      if (step && step.artifactName && step.stage === fromStage && step.status === "completed") {
        // Check if the next step targets the toStage
        var nextStep = run.steps[j + 1];
        if (nextStep && nextStep.stage === toStage) {
          return step.artifactName;
        }
      }
    }
  }
  return null;
}

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
function findBottleneck(agents) {
  if (!agents) agents = appState.agents;
  var longest = null;
  var longestDuration = 0;
  var now = Date.now();
  var names = Object.keys(agents);
  for (var i = 0; i < names.length; i++) {
    var agent = agents[names[i]];
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
      var prevStage = STAGE_ORDER[i - 1];
      var prevAgents = stageAgents[prevStage] || [];
      var prevActive = false;
      for (var p = 0; p < prevAgents.length; p++) {
        if (stateClass(prevAgents[p]) === "working") { prevActive = true; break; }
      }
      if (prevActive) conn.classList.add("active");
      // Add flowing animation if a run is transitioning through this connector
      if (runsAtStage(prevStage).length > 0 || runsAtStage(stage).length > 0) {
        conn.classList.add("flowing");
      }
      // Show artifact labels on connectors
      var artifactLabel = artifactBetweenStages(prevStage, stage);
      if (artifactLabel) {
        var label = el("span", "artifact-label", artifactLabel);
        conn.appendChild(label);
      }
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

      // Click + keyboard handler — enter focus mode
      card.setAttribute("tabindex", "0");
      card.setAttribute("role", "button");
      card.addEventListener("click", (function(ws, ag) {
        return function() { enterFocusMode(ws, ag); };
      })(primary.workspace, primary.agent));
      card.addEventListener("keydown", (function(ws, ag) {
        return function(e) {
          if (e.key === "Enter" || e.key === " ") { e.preventDefault(); enterFocusMode(ws, ag); }
        };
      })(primary.workspace, primary.agent));
    }

    // Render work-item dots for active runs at this stage
    var stageRuns = runsAtStage(stage);
    if (stageRuns.length > 0) {
      var dotsRow = el("div", "run-dots");
      for (var r = 0; r < stageRuns.length; r++) {
        var dot = el("button", "run-dot");
        if (stageRuns[r].status === "failed") dot.classList.add("run-dot-failed");
        else dot.classList.add("run-dot-active");
        if (appState.selectedRun === stageRuns[r].id) dot.classList.add("run-dot-selected");
        dot.title = stageRuns[r].workflowName + " (step " + stageRuns[r].stepIndex + "/" + stageRuns[r].totalSteps + ")";
        dot.addEventListener("click", (function(rid) {
          return function(e) { e.stopPropagation(); selectRun(rid); };
        })(stageRuns[r].id));
        dotsRow.appendChild(dot);
      }
      card.appendChild(dotsRow);
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

  // Run count in HUD
  var runIds = Object.keys(appState.runs);
  var runningCount = 0;
  for (i = 0; i < runIds.length; i++) {
    if (appState.runs[runIds[i]].status === "running") runningCount++;
  }
  var $hudRuns = document.getElementById("hud-runs");
  if ($hudRuns) {
    if (runningCount > 0) {
      // Show run count + current stage of first running run
      var runStageLabel = "";
      for (i = 0; i < runIds.length; i++) {
        var r = appState.runs[runIds[i]];
        if (r.status === "running" && r.stage) {
          runStageLabel = " \u2192 " + r.stage + " " + r.stepIndex + "/" + r.totalSteps;
          break;
        }
      }
      $hudRuns.textContent = runningCount + " run" + (runningCount > 1 ? "s" : "") + runStageLabel;
      $hudRuns.className = "hud-item active-run";
    } else {
      $hudRuns.textContent = "";
      $hudRuns.className = "hud-item";
    }
  }

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
    var label = el("span", null, meta[i][0] + ":");
    s.appendChild(label);
    s.appendChild(document.createTextNode(" " + meta[i][1]));
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
  appState.selectedRun = null;
  $drawer.classList.remove("open");
  renderPipeline();
}

$detailClose.addEventListener("click", closeDrawer);

// ── Run detail drawer (step timeline) ──
function selectRun(runId) {
  if (appState.selectedRun === runId) {
    closeDrawer();
    return;
  }
  appState.selectedRun = runId;
  appState.selectedAgent = null;
  var run = appState.runs[runId];
  if (!run) { closeDrawer(); return; }

  $detailName.textContent = run.workflowName + " — " + run.status;

  // Meta info
  $detailMeta.innerHTML = "";
  var meta = [
    ["run", runId.substring(0, 12)],
    ["status", run.status],
    ["step", run.stepIndex + "/" + run.totalSteps],
    ["started", timeAgo(run.startedAt)],
  ];
  if (run.finishedAt) meta.push(["finished", timeAgo(run.finishedAt)]);
  if (run.stage) meta.push(["stage", run.stage]);
  for (var i = 0; i < meta.length; i++) {
    var s = el("span", null, null);
    var label = el("span", null, meta[i][0] + ":");
    s.appendChild(label);
    s.appendChild(document.createTextNode(" " + meta[i][1]));
    $detailMeta.appendChild(s);
  }

  // Step timeline
  while ($detailEvts.firstChild) $detailEvts.removeChild($detailEvts.firstChild);
  var hasSteps = false;
  for (var j = 1; j <= run.totalSteps; j++) {
    var step = run.steps[j];
    if (!step) continue;
    hasSteps = true;
    var li = document.createElement("li");
    li.className = "step-row";

    // Step index badge
    var badge = el("span", "step-badge", String(step.index));
    if (step.status === "completed") badge.classList.add("step-ok");
    else if (step.status === "failed") badge.classList.add("step-fail");
    else badge.classList.add("step-running");
    li.appendChild(badge);

    // Step type + agent
    var desc = step.type;
    if (step.agent) desc += " \u2192 " + step.agent;
    li.appendChild(el("span", "step-desc", desc));

    // Duration
    if (step.durationMs !== null) {
      li.appendChild(el("span", "step-dur", formatDuration(step.durationMs)));
    } else if (step.status === "running") {
      li.appendChild(el("span", "step-dur running-text", "running\u2026"));
    }

    // Failure message
    if (step.status === "failed" && step.message) {
      var msg = el("div", "step-msg", step.message);
      li.appendChild(msg);
    }

    $detailEvts.appendChild(li);
  }
  if (!hasSteps) {
    $detailEvts.appendChild(el("li", null, "no steps recorded yet"));
  }

  $drawer.classList.add("open");
  renderPipeline();
}

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

  var wasConnected = false;

  es.onopen = function() {
    $connDot.className = "conn-dot connected";
    // On reconnect, refetch health snapshot only — don't replay the full
    // event log into live state as it can duplicate run entries and refire
    // side effects. The SSE stream will deliver any events missed during
    // the disconnection window.
    if (wasConnected) {
      fetchHealth();
    }
    wasConnected = true;
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

      // Track workflow runs
      if (data.event && data.event.indexOf("workflow.") === 0) {
        processWorkflowEvent(data);
      }

      scheduleRender();
    } catch (_) { /* ignore parse errors */ }
  };

  // Event types actually emitted by the server
  var eventTypes = [
    "agent.started", "agent.stopped",
    "agent.working", "agent.idle",
    "agent.auth_failed", "agent.auth_recovered",
    "agent.rate_limited", "agent.provider_down", "agent.provider_recovered",
    "workflow.started", "workflow.completed", "workflow.failed",
    "workflow.step.started", "workflow.step.completed", "workflow.step.failed"
  ];
  for (var i = 0; i < eventTypes.length; i++) {
    es.addEventListener(eventTypes[i], handler);
  }

  // Also listen for unnamed "message" events
  es.onmessage = handler;
}

// ── Agent Focus Mode ──
var $focusView     = document.getElementById("focus-view");
var $focusBack     = document.getElementById("focus-back");
var $focusAgentName = document.getElementById("focus-agent-name");
var $focusStatus   = document.getElementById("focus-status");
var $focusMeta     = document.getElementById("focus-meta");
var $focusTerminal = document.getElementById("focus-terminal");
var $focusStats    = document.getElementById("focus-stats");
var $focusDiff     = document.getElementById("focus-diff");
var $focusProgress = document.getElementById("focus-progress");
var $focusInput    = document.getElementById("focus-prompt-input");
var $focusSend     = document.getElementById("focus-send");
var focusPollId    = null;
var focusUsagePollId = null;
var focusPolling   = false;

function enterFocusMode(workspace, agent) {
  appState.view = "focus";
  appState.focusAgent = { workspace: workspace, agent: agent };
  appState.selectedAgent = null;

  // Hide factory, show focus
  document.getElementById("factory").style.display = "none";
  document.getElementById("detail-drawer").style.display = "none";
  document.getElementById("dispatch-panel").style.display = "none";
  document.getElementById("timeline").style.display = "none";
  $focusView.style.display = "flex";

  $focusAgentName.textContent = agent;

  // Pre-populate from cached health data for instant render
  var key = agentKey(workspace, agent);
  var cached = appState.agents[key];
  if (cached) {
    var sc = stateClass(cached);
    $focusStatus.textContent = "\u25CF " + stateLabel(cached);
    $focusStatus.className = "focus-status " + sc;
    $focusMeta.textContent = cached.session_name || "";
  }
  $focusTerminal.textContent = (cached && cached.running) ? "Loading terminal\u2026" : "";
  $focusStats.innerHTML = "";
  $focusDiff.innerHTML = '<div class="focus-diff-empty">Loading\u2026</div>';
  $focusProgress.innerHTML = "";

  // Start polling — first call fires immediately (fast: terminal + diff)
  pollFocus();
  focusPollId = setInterval(pollFocus, 2000);
  // Usage poll on slower cadence (expensive filesystem scan)
  pollFocusUsage();
  focusUsagePollId = setInterval(pollFocusUsage, 30000);
}

function exitFocusMode() {
  appState.view = "factory";
  appState.focusAgent = null;
  if (focusPollId) { clearInterval(focusPollId); focusPollId = null; }
  if (focusUsagePollId) { clearInterval(focusUsagePollId); focusUsagePollId = null; }

  $focusView.style.display = "none";
  document.getElementById("factory").style.display = "";
  document.getElementById("dispatch-panel").style.display = "";
  document.getElementById("timeline").style.display = "";

  renderPipeline();
  renderTimeline();
}

if ($focusBack) $focusBack.addEventListener("click", exitFocusMode);

// Keyboard: Escape exits focus mode
document.addEventListener("keydown", function(e) {
  if (e.key === "Escape" && appState.view === "focus") exitFocusMode();
});

function pollFocus() {
  var fa = appState.focusAgent;
  if (!fa) return;
  if (focusPolling) return;
  focusPolling = true;
  var url = "/v1/agents/" + encodeURIComponent(fa.workspace) + "/" + encodeURIComponent(fa.agent) + "/focus?lines=200";
  fetch(url).then(function(res) { return res.json(); }).then(function(json) {
    focusPolling = false;
    if (!appState.focusAgent || appState.focusAgent.workspace !== fa.workspace || appState.focusAgent.agent !== fa.agent) return;
    if (!json.data) return;
    renderFocusView(json.data);
  }).catch(function() {
    focusPolling = false;
    if (appState.focusAgent && appState.focusAgent.agent === fa.agent) {
      $focusTerminal.textContent = "Connection lost. Reconnecting\u2026";
    }
  });
}

// Separate slow usage poll (filesystem scan takes ~10s)
function pollFocusUsage() {
  var fa = appState.focusAgent;
  if (!fa) return;
  var url = "/v1/agents/" + encodeURIComponent(fa.workspace) + "/" + encodeURIComponent(fa.agent) + "/focus?lines=1&usage=1";
  fetch(url).then(function(res) { return res.json(); }).then(function(json) {
    if (!appState.focusAgent || appState.focusAgent.agent !== fa.agent) return;
    if (!json.data || !json.data.usage) return;
    // Update just the usage stats section
    var u = json.data.usage;
    var statsHtml = "";
    statsHtml += statRow("input tokens", formatTokens(u.input_tokens || 0));
    statsHtml += statRow("output tokens", formatTokens(u.output_tokens || 0));
    statsHtml += statRow("cache read", formatTokens(u.cache_read || 0), "green");
    statsHtml += statRow("cache write", formatTokens(u.cache_write || 0));
    $focusStats.innerHTML = statsHtml;
  }).catch(function() { /* silently ignore usage poll errors */ });
}

function renderFocusView(data) {
  // Status bar
  var stateStr = data.running ? "working" : "stopped";
  $focusStatus.textContent = "\u25CF " + stateStr;
  $focusStatus.className = "focus-status " + stateStr;
  $focusMeta.textContent = data.session || "";

  // Terminal
  if (!data.running && !data.terminal) {
    $focusTerminal.innerHTML = "";
    var emptyDiv = el("div", "term-empty", "Agent is not running.");
    var startBtn = document.createElement("button");
    startBtn.textContent = "Start Agent";
    startBtn.addEventListener("click", function() {
      var fa = appState.focusAgent;
      if (!fa) return;
      startBtn.disabled = true;
      startBtn.textContent = "Starting\u2026";
      fetch("/v1/actions/up", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ agent: fa.agent, workspace: fa.workspace })
      }).then(function(res) {
          if (!res.ok) { startBtn.textContent = "Failed"; startBtn.disabled = false; }
          else { startBtn.textContent = "Started"; }
        })
        .catch(function() { startBtn.textContent = "Failed"; startBtn.disabled = false; });
    });
    emptyDiv.appendChild(startBtn);
    $focusTerminal.appendChild(emptyDiv);
  } else {
    // Smart auto-scroll: only scroll if user is at the bottom
    var termEl = $focusTerminal;
    var wasAtBottom = termEl.scrollHeight - termEl.scrollTop - termEl.clientHeight < 30;
    termEl.textContent = data.terminal || "";
    if (wasAtBottom) termEl.scrollTop = termEl.scrollHeight;
  }

  // Usage stats
  var u = data.usage || {};
  var statsHtml = "";
  statsHtml += statRow("input tokens", formatTokens(u.input_tokens || 0));
  statsHtml += statRow("output tokens", formatTokens(u.output_tokens || 0));
  statsHtml += statRow("cache read", formatTokens(u.cache_read || 0), "green");
  statsHtml += statRow("cache write", formatTokens(u.cache_write || 0));
  // Context bar
  var ctxPct = data.context_pct;
  if (ctxPct != null) {
    var ctxColor = ctxPct <= 70 ? "green" : (ctxPct <= 90 ? "amber" : "red");
    statsHtml += statRow("context", ctxPct + "%", ctxColor);
    statsHtml += '<div class="focus-ctx-bar"><div class="focus-ctx-fill" style="width:' + ctxPct + '%;background:var(--' + (ctxColor === "green" ? "working" : ctxColor === "amber" ? "auth-fail" : "blocked") + ')"></div></div>';
  } else {
    statsHtml += statRow("context", "\u2014");
  }
  $focusStats.innerHTML = statsHtml;

  // Diff
  var d = data.diff || {};
  if (d.text) {
    var diffHtml = "";
    var lines = d.text.split("\n");
    for (var i = 0; i < lines.length && i < 100; i++) {
      var line = lines[i];
      var cls = "";
      if (line.indexOf("+") === 0 && line.indexOf("+++") !== 0) cls = "focus-diff-add";
      else if (line.indexOf("-") === 0 && line.indexOf("---") !== 0) cls = "focus-diff-del";
      else if (line.indexOf("@@") === 0) cls = "focus-diff-hunk";
      else if (line.indexOf("diff ") === 0) cls = "focus-diff-file";
      diffHtml += '<div class="' + cls + '">' + escapeHtml(line) + '</div>';
    }
    if (lines.length > 100) diffHtml += '<div class="focus-diff-empty">\u2026 ' + (lines.length - 100) + ' more lines</div>';
    diffHtml += '<div style="margin-top:4px;font-size:9px;color:var(--dim)">' + (d.files_changed || 0) + ' files, +' + (d.insertions || 0) + ' -' + (d.deletions || 0) + '</div>';
    $focusDiff.innerHTML = diffHtml;
  } else {
    $focusDiff.innerHTML = '<div class="focus-diff-empty">No changes yet.</div>';
  }

  // Progress — show active run for this agent if any
  var progressHtml = "";
  var runIds = Object.keys(appState.runs);
  for (var j = 0; j < runIds.length; j++) {
    var run = appState.runs[runIds[j]];
    if (run.status === "running") {
      progressHtml += statRow("workflow", run.workflowName);
      progressHtml += statRow("step", run.stepIndex + " / " + run.totalSteps);
      progressHtml += statRow("started", timeAgo(run.startedAt));
      break;
    }
  }
  if (!progressHtml) {
    progressHtml = '<div class="focus-diff-empty">No active run.</div>';
  }
  $focusProgress.innerHTML = progressHtml;
}

function statRow(label, value, colorClass) {
  return '<div class="focus-stat-row"><span class="focus-stat-label">' + escapeHtml(label) + '</span><span class="focus-stat-value' + (colorClass ? ' ' + colorClass : '') + '">' + escapeHtml(String(value)) + '</span></div>';
}

function formatTokens(n) {
  if (!n) return "0";
  if (n >= 1000000) return (n / 1000000).toFixed(1) + "M";
  if (n >= 1000) return (n / 1000).toFixed(1) + "K";
  return String(n);
}

function escapeHtml(str) {
  return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// Prompt send from focus view
if ($focusSend) {
  $focusSend.addEventListener("click", sendFocusPrompt);
}
if ($focusInput) {
  $focusInput.addEventListener("keydown", function(e) {
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); sendFocusPrompt(); }
  });
}

function sendFocusPrompt() {
  var fa = appState.focusAgent;
  if (!fa || !$focusInput.value.trim()) return;
  var prompt = $focusInput.value.trim();
  $focusSend.disabled = true;
  $focusSend.classList.add("sending");

  fetch("/v1/actions/send", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ agent: fa.agent, workspace: fa.workspace, prompt: prompt, auto_up: true })
  }).then(function(res) { return res.json(); }).then(function(json) {
    $focusSend.disabled = false;
    $focusSend.classList.remove("sending");
    if (json.ok) {
      $focusInput.value = "";
      $focusInput.style.borderColor = "var(--working)";
      setTimeout(function() { $focusInput.style.borderColor = ""; }, 1000);
    } else {
      $focusInput.style.borderColor = "var(--blocked)";
      setTimeout(function() { $focusInput.style.borderColor = ""; }, 2000);
    }
  }).catch(function() {
    $focusSend.disabled = false;
    $focusSend.classList.remove("sending");
    $focusInput.style.borderColor = "var(--blocked)";
    setTimeout(function() { $focusInput.style.borderColor = ""; }, 2000);
  });
}

// ── Dispatch panel ──
var $dispatchToggle = document.getElementById("dispatch-toggle");
var $dispatchForm   = document.getElementById("dispatch-form");
var $dispatchWf     = document.getElementById("dispatch-workflow");
var $dispatchIssue  = document.getElementById("dispatch-issue");
var $dispatchGo     = document.getElementById("dispatch-go");
var $dispatchStatus = document.getElementById("dispatch-status");

if ($dispatchToggle) {
  $dispatchToggle.addEventListener("click", function() {
    $dispatchForm.classList.toggle("open");
    if ($dispatchForm.classList.contains("open") && $dispatchWf.options.length <= 1) {
      loadWorkflows();
    }
  });
}

function loadWorkflows() {
  fetch("/v1/workflows").then(function(res) { return res.json(); }).then(function(json) {
    var wfs = (json.data && json.data.workflows) || json.data || [];
    $dispatchWf.innerHTML = "";
    if (wfs.length === 0) {
      $dispatchWf.appendChild(new Option("no workflows", ""));
      return;
    }
    for (var i = 0; i < wfs.length; i++) {
      var name = typeof wfs[i] === "string" ? wfs[i] : (wfs[i].name || "");
      if (name) $dispatchWf.appendChild(new Option(name, name));
    }
  }).catch(function() {
    $dispatchWf.innerHTML = "";
    $dispatchWf.appendChild(new Option("error loading", ""));
  });
}

if ($dispatchGo) {
  $dispatchGo.addEventListener("click", function() {
    var wf = $dispatchWf.value;
    if (!wf) return;
    $dispatchGo.disabled = true;
    $dispatchStatus.textContent = "dispatching…";
    $dispatchStatus.className = "dispatch-status";

    var body = { workflow: wf };
    var issue = ($dispatchIssue.value || "").trim();
    if (issue) body.issue = issue;

    fetch("/v1/actions/run", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body)
    }).then(function(res) { return res.json(); }).then(function(json) {
      $dispatchGo.disabled = false;
      if (json.ok) {
        $dispatchStatus.textContent = "dispatched";
        $dispatchStatus.className = "dispatch-status ok";
      } else {
        $dispatchStatus.textContent = (json.error && json.error.message) || "failed";
        $dispatchStatus.className = "dispatch-status err";
      }
    }).catch(function(e) {
      $dispatchGo.disabled = false;
      $dispatchStatus.textContent = "error: " + e.message;
      $dispatchStatus.className = "dispatch-status err";
    });
  });
}

// ── Historical run reconstruction ──
// Fetch past events from /v1/events and replay workflow events to rebuild run state
function reconstructRuns() {
  return fetch("/v1/events").then(function(res) {
    return res.json();
  }).then(function(json) {
    var events = json.data || [];
    // Events come oldest-first from the API; replay in order
    for (var i = 0; i < events.length; i++) {
      var evt = events[i];
      if (evt.event && evt.event.indexOf("workflow.") === 0) {
        processWorkflowEvent(evt);
      }
      // Also populate the event timeline (newest first)
      appState.events.unshift(evt);
      if (appState.events.length > 50) appState.events.length = 50;
    }
    // Clean up orphan runs: if a run was "started" but no terminal event
    // was found in the event window, mark it as stale. The /v1/events
    // endpoint only returns recent events, so old runs without a matching
    // completed/failed event are zombies.
    var runIds = Object.keys(appState.runs);
    var cutoff = Date.now() - 30 * 60 * 1000; // 30 min age threshold
    for (var j = 0; j < runIds.length; j++) {
      var run = appState.runs[runIds[j]];
      if (run.status === "running") {
        var startedMs = new Date(run.startedAt).getTime();
        if (startedMs < cutoff) {
          delete appState.runs[runIds[j]];
        }
      }
    }
    scheduleRender();
  }).catch(function(e) {
    console.warn("event reconstruction failed:", e);
  });
}

// ── Boot ──
fetchHealth().then(function() {
  return reconstructRuns();
}).then(function() {
  connectSSE();
  // Re-fetch health periodically to stay in sync
  setInterval(fetchHealth, 15000);
});
