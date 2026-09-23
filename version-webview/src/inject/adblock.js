// Quita los anuncios de las respuestas del reproductor antes de que la pagina las use.
// Port de plugins/do-not-track/injectors/inject.ts de Pear (el "pruner").
// Corre al crear el documento, antes que cualquier script de YouTube.
const AD_KEYS = ['adPlacements', 'playerAds', 'adSlots', 'adBreakHeartbeatParams'];
const adblockOn = () => !(window.__yir && window.__yir.cfg && window.__yir.cfg.adblock === false);

const prune = (o) => {
  if (!o || typeof o !== 'object') return o;
  for (const k of AD_KEYS) if (k in o) delete o[k];
  if (o.playerResponse) prune(o.playerResponse);
  if (o.ytInitialPlayerResponse) prune(o.ytInitialPlayerResponse);
  return o;
};
const hasAds = (o) =>
  o && typeof o === 'object' &&
  (AD_KEYS.some((k) => k in o) || 'playerResponse' in o || 'ytInitialPlayerResponse' in o);

const origParse = JSON.parse;
JSON.parse = function (text, reviver) {
  const r = origParse.call(this, text, reviver);
  try { if (adblockOn() && hasAds(r)) prune(r); } catch (_) {}
  return r;
};

const origJson = Response.prototype.json;
Response.prototype.json = function () {
  return origJson.call(this).then((r) => {
    try { if (adblockOn() && hasAds(r)) prune(r); } catch (_) {}
    return r;
  });
};

let initialPlayerResponse;
try {
  Object.defineProperty(window, 'ytInitialPlayerResponse', {
    configurable: true,
    get: () => initialPlayerResponse,
    set: (v) => { initialPlayerResponse = adblockOn() ? prune(v) : v; },
  });
} catch (_) {}
