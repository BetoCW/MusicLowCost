// Puente entre YouTube Music y Rust + port de los plugins "de pagina" de Pear Desktop.
// Ojo: YouTube Music exige Trusted Types, asi que NUNCA usar innerHTML aqui;
// todo el DOM se crea con createElement/textContent.

const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args || {});

const yir = (window.__yir = window.__yir || {});
yir.cfg = null;
yir.api = null;
yir.song = null;

const log = (level, ...parts) => {
  const msg = parts.map((p) => (typeof p === 'string' ? p : safeJson(p))).join(' ');
  invoke('yir_log', { level, msg }).catch(() => {});
};
const safeJson = (v) => { try { return JSON.stringify(v); } catch (_) { return String(v); } };
window.addEventListener('error', (e) => log('error', 'JS:', e.message, e.filename + ':' + e.lineno));
window.addEventListener('unhandledrejection', (e) => log('error', 'Promise:', String(e.reason)));

// ---------- utilidades DOM ----------
const h = (tag, props, ...children) => {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (k === 'style') el.style.cssText = v;
    else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (k === 'text') el.textContent = v;
    else if (k in el) el[k] = v;
    else el.setAttribute(k, v);
  }
  for (const c of children.flat()) if (c != null) el.append(c);
  return el;
};
const addStyle = (css, id) => {
  const st = (id && document.getElementById(id)) || h('style', id ? { id } : {});
  st.textContent = css;
  if (!st.isConnected) (document.head || document.documentElement).append(st);
  return st;
};
const waitFor = (selector, test = (el) => !!el) =>
  new Promise((resolve) => {
    const check = () => {
      const el = document.querySelector(selector);
      if (el && test(el)) { resolve(el); return true; }
      return false;
    };
    if (check()) return;
    const obs = new MutationObserver(() => { if (check()) obs.disconnect(); });
    obs.observe(document.documentElement, { childList: true, subtree: true });
  });
const video = () => document.querySelector('video');
const playerBar = () => document.querySelector('ytmusic-player-bar');

// ---------- ordenes que llegan desde Rust (bandeja, teclas multimedia, API) ----------
yir.command = (cmd, arg) => {
  const api = yir.api;
  switch (cmd) {
    case 'play': api && api.playVideo(); break;
    case 'pause': api && api.pauseVideo(); break;
    case 'toggle':
      if (!api) break;
      if (api.getPlayerState() === 1) api.pauseVideo(); else api.playVideo();
      break;
    case 'next': document.querySelector('.next-button.ytmusic-player-bar')?.click(); break;
    case 'previous': document.querySelector('.previous-button.ytmusic-player-bar')?.click(); break;
    case 'like': document.querySelector('#like-button-renderer')?.updateLikeStatus?.('LIKE'); break;
    case 'dislike': document.querySelector('#like-button-renderer')?.updateLikeStatus?.('DISLIKE'); break;
    case 'seekTo': api && api.seekTo(Number(arg)); break;
    case 'seekBy': api && api.seekBy(Number(arg)); break;
    case 'openSettings': openSettings(); break;
    case 'reload': location.reload(); break;
  }
};

// ---------- informacion de la cancion (port de providers/song-info-front.ts) ----------
const findPlayerOverlays = (videoData) => {
  for (const v of Object.values(videoData || {})) {
    if (v && typeof v === 'object' && v.playerOverlays) return v.playerOverlays;
  }
  try { return yir.api.getWatchNextResponse?.()?.playerOverlays; } catch (_) { return undefined; }
};

const albumFromDom = () => {
  const link = [...document.querySelectorAll('ytmusic-player-bar .byline a')]
    .find((a) => (a.getAttribute('href') || '').includes('browse/MPREb'));
  return link ? link.textContent.trim() : undefined;
};

const sendSongInfo = (videoData) => {
  const api = yir.api;
  let details;
  try { details = api.getPlayerResponse().videoDetails; } catch (_) { return; }
  if (!details || !details.videoId) return;
  const overlays = findPlayerOverlays(videoData);
  const album =
    overlays?.playerOverlayRenderer?.browserMediaSession?.browserMediaSessionRenderer?.album?.runs?.[0]?.text ||
    albumFromDom();
  const thumbs = details.thumbnail?.thumbnails || [];
  const song = {
    videoId: details.videoId,
    title: details.title || '',
    artist: (details.author || '').replace(/ - Topic$/, ''),
    album: album || null,
    duration: Number(details.lengthSeconds) || video()?.duration || 0,
    thumbnail: thumbs.length ? thumbs[thumbs.length - 1].url.split('?')[0] : null,
  };
  yir.song = song;
  invoke('yir_song_changed', { song }).catch((e) => log('warn', 'song_changed', String(e)));
  document.dispatchEvent(new CustomEvent('yir:song', { detail: song }));
};

const sendPlayState = () => {
  const v = video();
  if (!v) return;
  invoke('yir_play_state', { paused: v.paused, elapsed: v.currentTime || 0 }).catch(() => {});
};

const setupSongInfo = (api) => {
  const waiting = new Map();
  api.addEventListener('videodatachange', (name, videoData) => {
    document.dispatchEvent(new CustomEvent('yir:videodatachange', { detail: { name, videoData } }));
    const id = videoData && videoData.videoId;
    if (!id) return;
    // "dataupdated" trae el album; si no llega en 1.5 s usamos "dataloaded" (igual que Pear).
    if (name === 'dataupdated' && waiting.has(id)) {
      clearTimeout(waiting.get(id));
      waiting.delete(id);
      sendSongInfo(videoData);
    } else if (name === 'dataloaded') {
      clearTimeout(waiting.get(id));
      waiting.set(id, setTimeout(() => { waiting.delete(id); sendSongInfo(videoData); }, 1500));
    }
  });

  const v = video();
  if (v) {
    for (const ev of ['playing', 'pause', 'seeked']) v.addEventListener(ev, sendPlayState);
    if (!isNaN(v.duration)) sendSongInfo({});
  }
};

// ---------- plugins de pagina ----------
const features = {};

// Volumen preciso con la rueda del mouse (plugins/precise-volume).
features.preciseVolume = (api, cfg) => {
  const hud = h('div', { id: 'yir-volume-hud' });
  document.body.append(hud);
  let hideTimer;
  const onWheel = (e) => {
    e.preventDefault();
    const step = cfg.preciseVolume.step || 2;
    let vol = api.getVolume() + (e.deltaY < 0 ? step : -step);
    vol = Math.max(0, Math.min(100, Math.round(vol)));
    api.setVolume(vol);
    if (vol > 0 && api.isMuted()) api.unMute();
    const slider = document.querySelector('#volume-slider');
    if (slider) slider.value = vol;
    hud.textContent = vol + '%';
    hud.style.opacity = '1';
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => { hud.style.opacity = '0'; }, 800);
  };
  for (const sel of ['ytmusic-player-bar', '#player']) {
    document.querySelector(sel)?.addEventListener('wheel', onWheel, { passive: false });
  }
};

// Curva de volumen exponencial (plugins/exponential-volume).
features.exponentialVolume = (api) => {
  const EXP = 3;
  const desc = Object.getOwnPropertyDescriptor(HTMLMediaElement.prototype, 'volume');
  const stored = new WeakMap();
  Object.defineProperty(HTMLMediaElement.prototype, 'volume', {
    get() {
      const calc = (desc.get.call(this) || 0) ** (1 / EXP);
      const s = stored.get(this) ?? 0;
      return Math.abs(s - calc) < 0.01 ? s : calc;
    },
    set(v) { stored.set(this, v); desc.set.call(this, v ** EXP); },
  });
  const sync = () => (api.getPlayerState() === 3 ? setTimeout(sync, 0) : api.setVolume(api.getVolume()));
  sync();
};

// No empezar a reproducir solo al abrir la app (plugins/disable-autoplay, modo "applyOnce").
features.disableAutoplay = (api) => {
  const handler = (e) => {
    if (e.detail.name !== 'dataloaded') return;
    document.removeEventListener('yir:videodatachange', handler);
    api.pauseVideo();
    video()?.addEventListener('timeupdate', (ev) => ev.target.pause(), { once: true });
  };
  document.addEventListener('yir:videodatachange', handler);
};

// Saltar canciones con "no me gusta" (plugins/skip-disliked-songs).
features.skipDisliked = async () => {
  const btn = await waitFor('#like-button-renderer');
  new MutationObserver(() => {
    if (btn.getAttribute('like-status') === 'DISLIKE') yir.command('next');
  }).observe(btn, { attributes: true, attributeFilter: ['like-status'] });
};

// Velocidad de reproduccion (plugins/playback-speed).
features.playbackSpeed = (_api, cfg) => {
  const speed = Number(cfg.playbackSpeed) || 1;
  const v = video();
  if (!v || speed === 1) return;
  const apply = () => { if (v.playbackRate !== speed) v.playbackRate = speed; };
  v.addEventListener('playing', apply);
  v.addEventListener('loadeddata', apply);
  apply();
};

// Ocultar el video y dejar solo la portada (plugins/video-toggle).
features.hideVideo = (api) => {
  addStyle(
    '#song-video.ytmusic-player{display:none!important}' +
      '#song-image.ytmusic-player{display:block!important}',
    'yir-hide-video',
  );
  const player = document.querySelector('ytmusic-player');
  const force = () => {
    if (player && player.getAttribute('playback-mode') !== 'ATV_PREFERRED') {
      player.setAttribute('playback-mode', 'ATV_PREFERRED');
    }
  };
  if (player) new MutationObserver(force).observe(player, { attributeFilter: ['playback-mode'] });
  document.addEventListener('yir:song', (e) => {
    force();
    const img = document.querySelector('#song-image img');
    if (img && e.detail.thumbnail && !img.src) img.src = e.detail.thumbnail;
  });
  force();
};

// Barra de navegacion con desenfoque (plugins/blur-nav-bar).
features.blurNavBar = () => {
  addStyle(
    '#nav-bar-background,#header.ytmusic-item-section-renderer{background:rgba(0,0,0,.3)!important;backdrop-filter:blur(8px)!important}' +
      'ytmusic-tabs{backdrop-filter:blur(8px)!important}ytmusic-tabs.stuck{background:rgba(0,0,0,.3)!important}' +
      '#nav-bar-divider{display:none!important}',
    'yir-blur-nav',
  );
};

// Ecualizador, compresor y saltar silencios comparten un solo AudioContext
// (plugins/equalizer, audio-compressor, skip-silences).
const EQ_FREQS = [60, 170, 310, 600, 1000, 3000, 6000, 12000, 14000, 16000];
features.audio = (_api, cfg) => {
  const v = video();
  if (!v) return;
  const ctx = new AudioContext();
  const src = ctx.createMediaElementSource(v);
  let node = src;
  if (cfg.equalizer.enabled) {
    yir.eqFilters = EQ_FREQS.map((f, i) => {
      const b = ctx.createBiquadFilter();
      b.type = i === 0 ? 'lowshelf' : i === EQ_FREQS.length - 1 ? 'highshelf' : 'peaking';
      b.frequency.value = f;
      b.Q.value = 1;
      b.gain.value = Number(cfg.equalizer.gains[i]) || 0;
      node.connect(b);
      node = b;
      return b;
    });
  }
  if (cfg.compressor) {
    const c = ctx.createDynamicsCompressor();
    c.threshold.value = -50;
    c.knee.value = 40;
    c.ratio.value = 12;
    c.attack.value = 0;
    c.release.value = 0.25;
    node.connect(c);
    node = c;
  }
  node.connect(ctx.destination);
  // El AudioContext puede nacer suspendido; sin esto no se escucharia nada.
  const resume = () => { if (ctx.state !== 'running') ctx.resume().catch(() => {}); };
  v.addEventListener('play', resume);
  document.addEventListener('click', resume, { once: true, capture: true });
  resume();

  if (cfg.skipSilences.enabled) skipSilences(ctx, src, v, cfg.skipSilences.onlyBeginning);
};

// Port de plugins/skip-silences/renderer.ts (analizador tipo "hark").
const skipSilences = (ctx, src, v, onlyBeginning) => {
  const analyser = ctx.createAnalyser();
  analyser.fftSize = 512;
  analyser.smoothingTimeConstant = 0.1;
  src.connect(analyser);
  const bins = new Float32Array(analyser.frequencyBinCount);
  const history = new Array(10).fill(0);
  let silent = false;
  let started = false;
  const THRESHOLD = -100;
  const maxVol = () => {
    analyser.getFloatFrequencyData(bins);
    let m = -Infinity;
    for (let i = 4; i < bins.length; i++) if (bins[i] > m && bins[i] < 0) m = bins[i];
    return m;
  };
  const skip = () => {
    if (onlyBeginning && started) return;
    if (silent && !v.paused) v.currentTime += 0.2;
  };
  const loop = () => {
    if (!v.paused) {
      const cur = maxVol();
      if (cur > THRESHOLD && silent) {
        if (history.slice(-3).reduce((a, b) => a + b, 0) >= 2) { silent = false; started = true; }
      } else if (cur < THRESHOLD && !silent) {
        if (history.every((x) => x === 0) && !(v.seeking || v.ended || v.muted || v.volume === 0)) {
          silent = true;
          skip();
        }
      }
      history.shift();
      history.push(cur > THRESHOLD ? 1 : 0);
    }
    // Pear revisa cada 2 ms; 20 ms basta y gasta mucha menos CPU.
    setTimeout(loop, v.paused ? 250 : 20);
  };
  const reset = () => { started = false; skip(); };
  v.addEventListener('play', reset);
  v.addEventListener('seeked', reset);
  loop();
};

// SponsorBlock: saltar partes que no son musica (plugins/sponsorblock).
features.sponsorblock = () => {
  const v = video();
  if (!v) return;
  let segments = [];
  document.addEventListener('yir:song', async (e) => {
    segments = [];
    try { segments = await invoke('yir_sponsor_segments', { videoId: e.detail.videoId }); } catch (_) {}
  });
  v.addEventListener('timeupdate', () => {
    for (const [start, end] of segments) {
      if (v.currentTime >= start && v.currentTime < end) { v.currentTime = end; break; }
    }
  });
  v.addEventListener('emptied', () => { segments = []; });
};

// Letras sincronizadas en un panel flotante (plugins/synced-lyrics).
features.lyrics = (_api, cfg) => {
  const v = video();
  const panel = h('div', { id: 'yir-lyrics' });
  const body = h('div', { class: 'yir-lyrics-body' });
  const title = h('div', { class: 'yir-lyrics-title', text: 'Letra' });
  panel.append(title, body);
  document.body.append(panel);

  let open = false;
  try { open = localStorage.getItem('yir-lyrics-open') === '1'; } catch (_) {}
  const setOpen = (o) => {
    open = o;
    panel.style.display = o ? 'flex' : 'none';
    toggle.classList.toggle('yir-active', o);
    try { localStorage.setItem('yir-lyrics-open', o ? '1' : '0'); } catch (_) {}
  };
  const toggle = h('button', { class: 'yir-btn', title: 'Letra sincronizada', text: 'Letra', onclick: () => setOpen(!open) });
  waitFor('ytmusic-player-bar .right-controls-buttons, ytmusic-player-bar #right-controls').then((el) => el.prepend(toggle));
  setOpen(open);

  let lines = [];
  let nodes = [];
  let current = -1;
  const setMessage = (msg) => { body.replaceChildren(h('div', { class: 'yir-lyrics-msg', text: msg })); lines = []; nodes = []; };

  document.addEventListener('yir:song', async (e) => {
    const s = e.detail;
    title.textContent = s.title + ' — ' + s.artist;
    setMessage('Buscando letra…');
    current = -1;
    try {
      const res = await invoke('yir_fetch_lyrics', {
        title: s.title, artist: s.artist, album: s.album, duration: s.duration,
      });
      if (yir.song !== s) return; // ya cambio de cancion
      if (!res) { setMessage('No se encontro letra'); return; }
      if (res.lines.length) {
        lines = res.lines;
        nodes = lines.map(([ms, text]) =>
          h('div', { class: 'yir-line', text: text || '♪', onclick: () => yir.api && yir.api.seekTo(ms / 1000) }),
        );
        body.replaceChildren(...nodes);
      } else {
        lines = [];
        nodes = [];
        body.replaceChildren(h('div', { class: 'yir-lyrics-plain', text: res.plain || '' }));
      }
    } catch (err) {
      setMessage('Error al buscar la letra');
      log('warn', 'lyrics', String(err));
    }
  });

  v?.addEventListener('timeupdate', () => {
    if (!open || !lines.length) return;
    const ms = v.currentTime * 1000 + 300;
    let lo = 0, hi = lines.length - 1, idx = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (lines[mid][0] <= ms) { idx = mid; lo = mid + 1; } else hi = mid - 1;
    }
    if (idx === current) return;
    nodes[current]?.classList.remove('yir-current');
    current = idx;
    const n = nodes[idx];
    if (n) {
      n.classList.add('yir-current');
      body.scrollTo({ top: n.offsetTop - body.clientHeight / 2 + n.clientHeight / 2, behavior: 'smooth' });
    }
  });
};

// Scripts de rendimiento de Pear (plugins/performance-improvement, MIT (c) CY Fung).
features.performance = () => {
  const hasGL = (() => {
    try { const c = document.createElement('canvas'); return !!(c.getContext('webgl') || c.getContext('experimental-webgl')); }
    catch (_) { return false; }
  })();
  try { injectRm3(); } catch (e) { log('warn', 'rm3', String(e)); }
  try { (hasGL ? injectCpuTamerByAnimationFrame : injectCpuTamerByDomMutation)(null); }
  catch (e) { log('warn', 'cpu-tamer', String(e)); }
};

// ---------- panel de ajustes (reemplaza el menu de plugins de Pear) ----------
const SETTINGS = [
  ['General', [
    ['general.closeToTray', 'Cerrar a la bandeja en vez de salir'],
    ['general.startMinimized', 'Iniciar minimizado en la bandeja'],
    ['general.lowMemoryWhenHidden', 'Liberar memoria al ocultar/minimizar'],
    ['general.lowMemoryFlags', 'Modo bajo consumo de Chromium (reinicia la app)'],
    ['performance', 'Mejoras de rendimiento (cpu-tamer + rm3)'],
    ['notifications', 'Notificacion al cambiar de cancion'],
  ]],
  ['Reproduccion', [
    ['adblock', 'Bloquear anuncios y rastreadores'],
    ['hideVideo', 'Ocultar video (solo portada)'],
    ['disableAutoplay', 'No reproducir al abrir la app'],
    ['skipDisliked', 'Saltar canciones con "No me gusta"'],
    ['playbackSpeed', 'Velocidad de reproduccion', 'number', { min: 0.25, max: 3, step: 0.05 }],
    ['preciseVolume.enabled', 'Volumen con la rueda del mouse'],
    ['preciseVolume.step', 'Paso de volumen (%)', 'number', { min: 1, max: 20, step: 1 }],
    ['exponentialVolume', 'Volumen exponencial (mas control a bajo volumen)'],
    ['sponsorblock.enabled', 'SponsorBlock (saltar intros/partes sin musica)'],
    ['lyrics.enabled', 'Letras sincronizadas (LRCLIB)'],
    ['lyrics.showInexact', 'Buscar letra aunque no coincida exacto'],
    ['blurNavBar', 'Barra superior con desenfoque'],
  ]],
  ['Audio (usa WebAudio)', [
    ['compressor', 'Compresor de audio (volumen parejo)'],
    ['skipSilences.enabled', 'Saltar silencios'],
    ['skipSilences.onlyBeginning', 'Solo el silencio del inicio'],
    ['equalizer.enabled', 'Ecualizador'],
    ['equalizer', 'Bandas', 'eq'],
  ]],
  ['Integraciones', [
    ['discord.enabled', 'Discord Rich Presence'],
    ['discord.hideWhenPaused', 'Ocultar en Discord al pausar'],
    ['discord.showButton', 'Boton "Escuchar" en Discord'],
    ['lastfm.enabled', 'Scrobbling a Last.fm'],
    ['lastfm', 'Cuenta Last.fm', 'lastfm'],
    ['listenbrainz.enabled', 'Scrobbling a ListenBrainz'],
    ['listenbrainz.token', 'Token de ListenBrainz', 'text'],
    ['apiServer.enabled', 'API local HTTP (127.0.0.1)'],
    ['apiServer.port', 'Puerto de la API', 'number', { min: 1024, max: 65535, step: 1 }],
  ]],
  ['Atajos globales', [
    ['shortcuts.enabled', 'Activar atajos globales'],
    ['shortcuts.playPause', 'Play / Pausa', 'text'],
    ['shortcuts.next', 'Siguiente', 'text'],
    ['shortcuts.previous', 'Anterior', 'text'],
    ['shortcuts.showHide', 'Mostrar / Ocultar ventana', 'text'],
  ]],
  ['Descargas (requiere yt-dlp)', [
    ['downloader.folder', 'Carpeta (vacio = Musica)', 'text'],
    ['downloader.format', 'Formato (mp3, m4a, opus, flac)', 'text'],
    ['download', 'Descargar la cancion actual', 'download'],
  ]],
];

const getPath = (o, p) => p.split('.').reduce((a, k) => (a == null ? a : a[k]), o);
const setPath = (o, p, v) => {
  const ks = p.split('.');
  const last = ks.pop();
  ks.reduce((a, k) => a[k], o)[last] = v;
};

let settingsEl = null;
const openSettings = () => {
  if (!yir.cfg) return;
  if (settingsEl) { settingsEl.remove(); settingsEl = null; return; }
  const draft = JSON.parse(JSON.stringify(yir.cfg));
  const status = h('span', { class: 'yir-status' });

  const row = ([path, label, type, opts]) => {
    if (type === 'eq') {
      const sliders = EQ_FREQS.map((f, i) => {
        const input = h('input', {
          type: 'range', min: -12, max: 12, step: 1, value: draft.equalizer.gains[i],
          oninput: (e) => {
            draft.equalizer.gains[i] = Number(e.target.value);
            val.textContent = e.target.value + ' dB';
            if (yir.eqFilters) yir.eqFilters[i].gain.value = Number(e.target.value);
          },
        });
        const val = h('small', { text: draft.equalizer.gains[i] + ' dB' });
        return h('label', { class: 'yir-eq-band' }, input, h('small', { text: f >= 1000 ? f / 1000 + 'k' : String(f) }), val);
      });
      return h('div', { class: 'yir-eq' }, ...sliders);
    }
    if (type === 'lastfm') {
      const connected = !!draft.lastfm.sessionKey;
      return h('div', { class: 'yir-row' },
        h('span', { text: connected ? 'Conectado a Last.fm' : 'Sin conectar' }),
        h('button', {
          class: 'yir-btn', text: connected ? 'Reconectar' : 'Conectar',
          onclick: async () => {
            status.textContent = 'Autoriza la app en el navegador…';
            try {
              const user = await invoke('yir_lastfm_connect');
              yir.cfg = await invoke('yir_get_config');
              draft.lastfm = yir.cfg.lastfm;
              status.textContent = 'Last.fm conectado' + (user ? ' como ' + user : '');
            } catch (e) { status.textContent = String(e); }
          },
        }));
    }
    if (type === 'download') {
      return h('div', { class: 'yir-row' }, h('button', {
        class: 'yir-btn', text: label,
        onclick: () => invoke('yir_download_current').then(
          () => { status.textContent = 'Descarga iniciada'; },
          (e) => { status.textContent = String(e); }),
      }));
    }
    const value = getPath(draft, path);
    if (type === 'text' || type === 'number') {
      const input = h('input', {
        type, value: value ?? '', ...(opts || {}),
        onchange: (e) => setPath(draft, path, type === 'number' ? Number(e.target.value) : e.target.value),
      });
      return h('label', { class: 'yir-row' }, h('span', { text: label }), input);
    }
    const input = h('input', { type: 'checkbox', checked: !!value, onchange: (e) => setPath(draft, path, e.target.checked) });
    return h('label', { class: 'yir-row' }, h('span', { text: label }), input);
  };

  const save = async (reload) => {
    try {
      await invoke('yir_set_config', { config: draft });
      yir.cfg = draft;
      if (reload) location.reload();
      else status.textContent = 'Guardado. Algunos cambios se aplican al recargar.';
    } catch (e) { status.textContent = 'Error: ' + e; }
  };

  settingsEl = h('div', { id: 'yir-settings', onclick: (e) => { if (e.target === settingsEl) openSettings(); } },
    h('div', { class: 'yir-panel' },
      h('div', { class: 'yir-head' },
        h('b', { text: 'YoutubeInRustWeb — Ajustes' }),
        h('button', { class: 'yir-btn', text: '✕', onclick: openSettings })),
      h('div', { class: 'yir-scroll' },
        ...SETTINGS.map(([name, rows]) => h('fieldset', {}, h('legend', { text: name }), ...rows.map(row)))),
      h('div', { class: 'yir-foot' },
        status,
        h('button', { class: 'yir-btn', text: 'Guardar', onclick: () => save(false) }),
        h('button', { class: 'yir-btn yir-primary', text: 'Guardar y recargar', onclick: () => save(true) }))));
  document.body.append(settingsEl);
};

const CSS = `
#yir-volume-hud{position:fixed;left:50%;bottom:90px;transform:translateX(-50%);z-index:9999;background:rgba(0,0,0,.75);color:#fff;
  padding:6px 14px;border-radius:14px;font:500 14px Roboto,sans-serif;opacity:0;transition:opacity .2s;pointer-events:none}
.yir-btn{background:#272727;color:#fff;border:1px solid #3a3a3a;border-radius:16px;padding:6px 12px;font:500 13px Roboto,sans-serif;cursor:pointer}
.yir-btn:hover{background:#3a3a3a}.yir-btn.yir-active,.yir-primary{background:#fff;color:#000}
#yir-gear{margin:0 8px;font-size:18px;padding:4px 10px}
#yir-settings{position:fixed;inset:0;z-index:10000;background:rgba(0,0,0,.6);display:flex;align-items:center;justify-content:center}
#yir-settings .yir-panel{background:#1d1d1d;color:#fff;width:min(620px,94vw);max-height:88vh;border-radius:12px;display:flex;flex-direction:column;
  font:14px Roboto,sans-serif;box-shadow:0 10px 40px rgba(0,0,0,.6)}
#yir-settings .yir-head,#yir-settings .yir-foot{display:flex;align-items:center;gap:8px;padding:12px 16px}
#yir-settings .yir-head{justify-content:space-between;border-bottom:1px solid #333}
#yir-settings .yir-foot{justify-content:flex-end;border-top:1px solid #333}
#yir-settings .yir-status{flex:1;color:#aaa;font-size:12px}
#yir-settings .yir-scroll{overflow:auto;padding:8px 16px}
#yir-settings fieldset{border:1px solid #333;border-radius:8px;margin:8px 0;padding:6px 12px}
#yir-settings legend{color:#aaa;padding:0 6px}
#yir-settings .yir-row{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:6px 0}
#yir-settings input[type=text],#yir-settings input[type=number]{background:#111;color:#fff;border:1px solid #444;border-radius:6px;padding:4px 8px;width:240px}
#yir-settings input[type=number]{width:90px}
#yir-settings .yir-eq{display:flex;justify-content:space-between;padding:6px 0}
#yir-settings .yir-eq-band{display:flex;flex-direction:column;align-items:center;gap:2px;font-size:11px;color:#aaa}
#yir-settings .yir-eq-band input{writing-mode:vertical-lr;direction:rtl;height:110px;width:22px}
#yir-lyrics{position:fixed;right:16px;bottom:88px;width:340px;height:min(460px,60vh);z-index:9000;background:rgba(15,15,15,.94);
  border:1px solid #333;border-radius:12px;display:none;flex-direction:column;color:#fff;font:15px Roboto,sans-serif}
#yir-lyrics .yir-lyrics-title{padding:10px 14px;font-weight:500;color:#aaa;border-bottom:1px solid #333;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
#yir-lyrics .yir-lyrics-body{flex:1;overflow:auto;padding:12px 14px;position:relative}
#yir-lyrics .yir-line{padding:5px 0;color:#777;cursor:pointer;transition:color .2s}
#yir-lyrics .yir-line.yir-current{color:#fff;font-weight:600;font-size:17px}
#yir-lyrics .yir-lyrics-plain{white-space:pre-wrap;color:#ddd;line-height:1.5}
#yir-lyrics .yir-lyrics-msg{color:#888;text-align:center;margin-top:30px}
`;

const addGearButton = async () => {
  const container = await waitFor('ytmusic-nav-bar #right-content');
  if (document.getElementById('yir-gear')) return;
  container.prepend(h('button', { id: 'yir-gear', class: 'yir-btn', title: 'Ajustes de YoutubeInRustWeb', text: '⚙', onclick: openSettings }));
};

// ---------- arranque ----------
const start = async () => {
  try {
    yir.cfg = await invoke('yir_get_config');
  } catch (e) {
    log('error', 'no se pudo leer la config', String(e));
    return;
  }
  const cfg = yir.cfg;
  if (cfg.performance) features.performance();

  await waitFor('body');
  addStyle(CSS, 'yir-css');
  addGearButton();
  document.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.key === ',') { e.preventDefault(); openSettings(); }
  });
  if (cfg.blurNavBar) features.blurNavBar();

  const api = await waitFor('#movie_player', (el) => typeof el.getPlayerResponse === 'function');
  yir.api = api;
  log('info', 'reproductor listo');

  const run = (name, on) => {
    if (!on) return;
    Promise.resolve()
      .then(() => features[name](api, cfg))
      .catch((e) => log('warn', 'plugin ' + name, String(e)));
  };
  // Los plugins se enganchan ANTES de setupSongInfo para recibir la primera cancion.
  run('disableAutoplay', cfg.disableAutoplay);
  run('preciseVolume', cfg.preciseVolume.enabled);
  run('exponentialVolume', cfg.exponentialVolume);
  run('skipDisliked', cfg.skipDisliked);
  run('playbackSpeed', Number(cfg.playbackSpeed) !== 1);
  run('hideVideo', cfg.hideVideo);
  run('sponsorblock', cfg.sponsorblock.enabled);
  run('lyrics', cfg.lyrics.enabled);
  run('audio', cfg.equalizer.enabled || cfg.compressor || cfg.skipSilences.enabled);
  await Promise.resolve();
  setupSongInfo(api);
};

if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true });
else start();
