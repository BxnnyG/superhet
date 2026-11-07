// Reference client for the Superhet panel bridge v1 (docs/plugins-ui-panels.md). The panel runs in
// a sandboxed frame without access to the web app, its cookies or the API; everything it knows
// arrives as messages from the host, and the only thing it can ask for is a tune request.
(function () {
  "use strict";
  var $ = function (id) {
    return document.getElementById(id);
  };
  var canTune = false;

  function send(message) {
    // The frame's origin is opaque, so the target origin is "*"; only the host window is
    // addressed (window.parent), never another origin.
    message.superhet = 1;
    window.parent.postMessage(message, "*");
  }

  function applyTheme(tokens) {
    var root = document.documentElement;
    Object.keys(tokens || {}).forEach(function (name) {
      root.style.setProperty("--sh-" + name, tokens[name]);
    });
  }

  function text(tag, value) {
    var el = document.createElement(tag);
    el.textContent = value; // never innerHTML: events carry text from the air
    return el;
  }

  function renderInstances(list) {
    var ul = $("instances");
    ul.replaceChildren();
    if (!list.length) ul.appendChild(text("li", "No instance running."));
    list.forEach(function (i) {
      var where = i.freq_hz ? " @ " + i.freq_hz + " Hz" : "";
      ul.appendChild(text("li", i.id + " - " + i.state + where));
    });
  }

  function renderEvents(list) {
    var ol = $("events");
    ol.replaceChildren();
    if (!list.length) ol.appendChild(text("li", "No events yet."));
    list.slice(0, 20).forEach(function (e) {
      var when = new Date(e.ts_ms).toISOString().slice(11, 19);
      ol.appendChild(text("li", when + "  " + e.kind + "  " + e.value));
    });
  }

  window.addEventListener("message", function (event) {
    // Only the host (the parent window) is believed.
    if (event.source !== window.parent) return;
    var m = event.data;
    if (!m || m.superhet !== 1) return;
    if (m.type === "init") {
      canTune = m.can_tune === true;
      $("tune").disabled = !canTune;
      $("tune-note").textContent = canTune ? "" : "Operators and admins only.";
      $("status").textContent = "Connected to " + m.plugin_id;
      applyTheme(m.theme);
    } else if (m.type === "theme") {
      applyTheme(m.theme);
    } else if (m.type === "instances") {
      renderInstances(m.instances);
    } else if (m.type === "events") {
      renderEvents(m.events);
    } else if (m.type === "tune_result") {
      $("tune-note").textContent = m.ok ? "Tuned." : "Refused: " + m.error;
    }
  });

  $("tune").addEventListener("click", function () {
    if (canTune) send({ type: "tune", freq_hz: 100000000 });
  });

  send({ type: "hello" });
})();
