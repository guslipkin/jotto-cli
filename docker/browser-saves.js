// Keeps the arcade's saves in this browser, so they outlive the session (whose
// own copy goes when it ends). The Dockerfile puts this in ttyd's page; the
// arcade, run with --browser-saves, talks to it through the terminal under
// OSC 7681 (see src/save.rs):
//
//   ESC ] 7681 ; load BEL                  -> typed back: "saves:" <base64> Enter
//   ESC ] 7681 ; save ; <game> ; <base64>  -> that game's section is kept
//
// The saves are kept a section per game and merged here, so two tabs playing
// different games don't overwrite each other. Storage that's full or turned
// off only means saves last the session, as without this.
(function () {
  var KEY = "arcade-saves";

  function stored() {
    try {
      var saves = JSON.parse(localStorage.getItem(KEY) || "{}");
      return saves && typeof saves === "object" && !Array.isArray(saves) ? saves : {};
    } catch (e) {
      return {};
    }
  }

  // Base64 of UTF-8, as the arcade's own encoding is.
  function encode(text) {
    var bytes = new TextEncoder().encode(text);
    var binary = "";
    for (var i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
    return btoa(binary);
  }

  function decode(base64) {
    var binary = atob(base64);
    var bytes = new Uint8Array(binary.length);
    for (var i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return new TextDecoder().decode(bytes);
  }

  function handle(term, data) {
    var parts = data.split(";");
    if (parts[0] === "load") {
      // Not as the player's own typing, so it neither scrolls nor clears a selection.
      term.input("saves:" + encode(JSON.stringify(stored())) + "\r", false);
    } else if (parts[0] === "save" && parts.length === 3) {
      try {
        var saves = stored();
        saves[parts[1]] = JSON.parse(decode(parts[2]));
        localStorage.setItem(KEY, JSON.stringify(saves));
      } catch (e) {}
    }
    return true;
  }

  // ttyd makes its terminal (window.term) as the page loads, before it connects,
  // so this is in place before the arcade, which starts on connecting, asks.
  function attach() {
    var term = window.term;
    if (!term || !term.parser || !term.input) {
      setTimeout(attach, 10);
      return;
    }
    term.parser.registerOscHandler(7681, function (data) {
      return handle(term, data);
    });
  }
  attach();
})();
