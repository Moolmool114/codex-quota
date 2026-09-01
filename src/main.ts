import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart';
import { animate, stagger } from 'motion';
import './style.css';

type WindowData = { kind: string; remaining_percent?: number; used_percent?: number; resets_at_ms?: number; window_duration_mins?: number };
type Snapshot = { five_hour?: WindowData; long_window?: WindowData; source: string; observed_at_ms?: number; received_at_ms?: number; source_file?: string };
type Settings = { window_opacity: number; background_opacity: number; text_opacity: number; animations_enabled: boolean; language: 'en' | 'zh-CN'; always_on_top: boolean; theme: string; start_with_windows: boolean; start_minimized: boolean; close_to_tray: boolean; refresh_interval_seconds: number; custom_session_path?: string };
type Diagnostics = { app_server_status: string; app_server_error?: string; app_server_last_read_ms?: number; app_server_last_notification_ms?: number; codex_executable: string; app_server_pid?: number; app_server_initialized: boolean; bucket?: string; watcher_status: string; session_path: string; source: string };

const defaults: Settings = { window_opacity: 1, background_opacity: 1, text_opacity: 1, animations_enabled: true, language: 'en', always_on_top: false, theme: 'system', start_with_windows: false, start_minimized: false, close_to_tray: true, refresh_interval_seconds: 30 };
const app = document.querySelector<HTMLDivElement>('#app')!;
let snapshot: Snapshot = { source: 'none' };
let settings = { ...defaults };
let refreshing = false;

app.className = 'app-shell';
app.innerHTML = `
  <div id="titlebar" class="titlebar" data-tauri-drag-region>
    <span class="brand"><b>◉</b> Codex Quota <em>0.6.1</em></span>
    <span class="window-actions" data-no-drag>
      <button id="pin" aria-label="Pin window" title="Pin window">⌖</button><button id="more" aria-label="Menu">⋯</button><button id="min" aria-label="Minimize">−</button><button id="close" aria-label="Close">×</button>
    </span>
  </div>
  <section id="home-view" class="view">
    <header><p data-i18n="viewer">LOCAL QUOTA VIEWER</p><button id="refresh">↻ Refresh</button></header>
    <div class="cards">
      <section class="card" id="five-card"><h2 data-i18n="fiveHours">5 HOURS</h2><div class="percent"><span id="five-percent">N/A</span><small data-i18n="remaining"> remaining</small></div><div class="bar"><i id="five-bar"></i></div><div class="label" data-i18n="resetIn">Reset in</div><div class="count" id="five-count">Waiting for sync</div><div class="muted" id="five-reset">Unavailable</div></section>
      <section class="card" id="long-card"><h2 id="long-title">LONG WINDOW</h2><div class="percent"><span id="long-percent">N/A</span><small data-i18n="remaining"> remaining</small></div><div class="bar"><i id="long-bar"></i></div><div class="label" data-i18n="resetIn">Reset in</div><div class="count" id="long-count">Waiting for sync</div><div class="muted" id="long-reset">Unavailable</div></section>
    </div>
    <footer><strong id="source-status">● No data</strong><span id="sync-status">Waiting for the first local sync</span></footer>
  </section>
  <section id="settings-view" class="view" hidden>
    <header><button id="back">← Back</button><p data-i18n="settings">SETTINGS</p></header>
    <div class="settings-panel">
      <h2 data-i18n="appearance">Appearance</h2>
      <label><span data-i18n="backgroundOpacity">Background opacity</span><output id="background-opacity-value">100%</output><input id="background-opacity" type="range" min="10" max="100" step="5"></label>
      <label><span data-i18n="textOpacity">Text opacity</span><output id="text-opacity-value">100%</output><input id="text-opacity" type="range" min="20" max="100" step="5"></label>
      <label><span data-i18n="theme">Theme</span><select id="theme"><option value="system">System</option><option value="dark">Dark</option><option value="light">Light</option><option value="glass">Glass (system)</option></select></label>
      <label><span data-i18n="language">Language</span><select id="language"><option value="en">English</option><option value="zh-CN">简体中文</option></select></label>
      <label class="check"><span data-i18n="animations">Animations</span><input id="animations" type="checkbox"></label>
      <h2 data-i18n="behavior">Behavior</h2>
      <label class="check"><span data-i18n="autostart">Start with Windows</span><input id="autostart" type="checkbox"></label>
      <label class="check"><span data-i18n="startMin">Start minimized</span><input id="start-min" type="checkbox"></label>
      <label class="check"><span data-i18n="closeTray">Close to tray</span><input id="close-tray" type="checkbox"></label>
      <h2 data-i18n="data">Data</h2>
      <label><span data-i18n="refreshInterval">Refresh interval</span><select id="interval"><option value="15">15 seconds</option><option value="30">30 seconds</option><option value="60">1 minute</option><option value="120">2 minutes</option><option value="300">5 minutes</option></select></label>
      <label><span data-i18n="sessionPath">Codex sessions path</span><input id="path" placeholder="Auto detect ~/.codex/sessions"></label>
      <div class="diagnostics"><b>Diagnostics</b><span id="diag-server">App server: starting</span><span id="diag-executable">Codex executable: checking</span><span id="diag-process">Initialized: no</span><span id="diag-read">Last full read: never</span><span id="diag-notification">Last notification: not observed</span><span id="diag-bucket">Bucket: unavailable</span><span id="diag-watcher">Session watcher: starting</span><span id="diag-source">Active source: none</span><span id="diag-error"></span></div>
      <button id="defaults">Reset defaults</button>
    </div>
  </section>
  <div id="menu" class="popup-menu" hidden><button id="menu-refresh">Refresh</button><button id="menu-settings">Settings</button><hr><button id="menu-quit">Quit</button></div>`;

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const messages = {
  en: { viewer: 'LOCAL QUOTA VIEWER', settings: 'SETTINGS', appearance: 'Appearance', backgroundOpacity: 'Background opacity', textOpacity: 'Text opacity', theme: 'Theme', language: 'Language', animations: 'Animations', behavior: 'Behavior', autostart: 'Start with Windows', startMin: 'Start minimized', closeTray: 'Close to tray', data: 'Data', refreshInterval: 'Refresh interval', sessionPath: 'Codex sessions path', fiveHours: '5 HOURS', weekly: 'WEEKLY', longWindow: 'LONG WINDOW', remaining: ' remaining', resetIn: 'Reset in', waitingSync: 'Waiting for sync', unavailable: 'Unavailable', refresh: '↻ Refresh', refreshing: 'Refreshing…', back: '← Back', defaults: 'Reset defaults', noData: '○ No data', waitingFirst: 'Waiting for the first local sync', observed: 'Observed', live: '● Live', sessionSource: '● Session', cached: '○ Cached', menuRefresh: 'Refresh', menuSettings: 'Settings', quit: 'Quit', pin: 'Pin window', unpin: 'Unpin window' },
  'zh-CN': { viewer: '本地用量查看器', settings: '设置', appearance: '外观', backgroundOpacity: '背景透明度', textOpacity: '文字透明度', theme: '主题', language: '语言', animations: '动态效果', behavior: '行为', autostart: '开机启动', startMin: '启动时最小化', closeTray: '关闭到托盘', data: '数据', refreshInterval: '刷新间隔', sessionPath: 'Codex 会话路径', fiveHours: '5 小时', weekly: '每周', longWindow: '长期窗口', remaining: ' 剩余', resetIn: '距离重置', waitingSync: '等待同步', unavailable: '不可用', refresh: '↻ 刷新', refreshing: '正在刷新…', back: '← 返回', defaults: '恢复默认设置', noData: '○ 暂无数据', waitingFirst: '等待首次本地同步', observed: '观测时间', live: '● 实时', sessionSource: '● 会话', cached: '○ 缓存', menuRefresh: '刷新', menuSettings: '设置', quit: '退出', pin: '窗口置顶', unpin: '取消置顶' }
} as const;
type MessageKey = keyof typeof messages.en;
const tr = (key: MessageKey) => messages[settings.language]?.[key] ?? messages.en[key];
const reducedMotion = () => !settings.animations_enabled || window.matchMedia('(prefers-reduced-motion: reduce)').matches;
const spring = { type: 'spring' as const, stiffness: 420, damping: 30 };
const easeOut = [0.22, 1, 0.36, 1] as const;

function motionText(element: HTMLElement, value: string) {
  if (element.textContent === value) return;
  element.textContent = value;
  if (!reducedMotion()) animate(element, { opacity: [0.35, 1], y: [3, 0], filter: ['blur(2px)', 'blur(0px)'] }, { duration: 0.28, ease: easeOut });
}

function enterElements(selector: string, y = 14) {
  if (reducedMotion()) return;
  const elements = document.querySelectorAll<HTMLElement>(selector);
  animate(elements, { opacity: [0, 1], y: [y, 0], scale: [0.97, 1] }, { duration: 0.55, delay: stagger(0.045), ease: easeOut });
}

async function closeMenu() {
  const menu = byId<HTMLElement>('menu');
  if (menu.hidden) return;
  if (!reducedMotion()) await animate(menu, { opacity: [1, 0], scale: [1, 0.94], y: [0, -6] }, { duration: 0.14, ease: 'easeIn' });
  menu.hidden = true;
}

function openMenu() {
  const menu = byId<HTMLElement>('menu');
  menu.hidden = false;
  if (!reducedMotion()) {
    animate(menu, { opacity: [0, 1], scale: [0.88, 1], y: [-8, 0] }, spring);
    animate(menu.querySelectorAll('button, hr'), { opacity: [0, 1], x: [8, 0] }, { duration: 0.3, delay: stagger(0.035), ease: easeOut });
  }
}
const sourceName = (source: string) => ({ 'app-server': 'Live app server', session: 'Local session', cache: 'Local cache', none: 'No data' }[source] ?? source);
const date = (value?: number) => value ? new Date(value).toLocaleString(settings.language) : tr('unavailable');
const relativeAge = (value?: number) => {
  if (!value) return 'unknown';
  const seconds = Math.max(0, Math.floor((Date.now() - value) / 1000));
  if (settings.language === 'zh-CN') {
    if (seconds < 5) return '刚刚';
    if (seconds < 60) return `${seconds} 秒前`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)} 分钟前`;
    return `${Math.floor(seconds / 3600)} 小时前`;
  }
  if (seconds < 5) return 'just now';
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  return `${Math.floor(seconds / 3600)}h ago`;
};
const countdown = (reset?: number) => {
  if (!reset) return tr('waitingSync');
  const seconds = Math.max(0, Math.floor((reset - Date.now()) / 1000));
  if (seconds === 0) return settings.language === 'zh-CN' ? '已到重置时间' : 'Reset reached';
  if (seconds >= 86400) return `${Math.floor(seconds / 86400)}d ${String(Math.floor(seconds % 86400 / 3600)).padStart(2, '0')}h ${String(Math.floor(seconds % 3600 / 60)).padStart(2, '0')}m`;
  return `${String(Math.floor(seconds / 3600)).padStart(2, '0')}:${String(Math.floor(seconds % 3600 / 60)).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`;
};

function updateWindow(prefix: 'five' | 'long', value?: WindowData) {
  const percent = value?.remaining_percent;
  const percentElement = byId(`${prefix}-percent`);
  const bar = byId<HTMLElement>(`${prefix}-bar`);
  motionText(percentElement, percent == null ? 'N/A' : `${Math.round(percent)}%`);
  const targetWidth = Math.max(0, Math.min(100, percent ?? 0));
  if (reducedMotion()) bar.style.width = `${targetWidth}%`;
  else animate(bar, { width: `${targetWidth}%` }, { duration: 0.75, ease: easeOut });
  motionText(byId(`${prefix}-count`), countdown(value?.resets_at_ms));
  motionText(byId(`${prefix}-reset`), date(value?.resets_at_ms));
  const card = byId(`${prefix}-card`);
  card.classList.toggle('unavailable', !value);
  if (!reducedMotion()) animate(card, { scale: [0.985, 1], filter: ['brightness(1.12)', 'brightness(1)'] }, { duration: 0.45, ease: easeOut });
}

function applySnapshot(value: Snapshot) {
  snapshot = value;
  updateWindow('five', value.five_hour);
  updateWindow('long', value.long_window);
  motionText(byId('long-title'), value.long_window?.kind === 'weekly' ? tr('weekly') : tr('longWindow'));
  updateSourceAge();
}

function updateSourceAge() {
  if (snapshot.source === 'app-server') motionText(byId('source-status'), `${tr('live')} · ${relativeAge(snapshot.received_at_ms)}`);
  else if (snapshot.source === 'session') motionText(byId('source-status'), `${tr('sessionSource')} · ${relativeAge(snapshot.observed_at_ms)}`);
  else if (snapshot.source === 'cache') motionText(byId('source-status'), `${tr('cached')} · ${snapshot.observed_at_ms ? new Date(snapshot.observed_at_ms).toLocaleTimeString(settings.language) : 'unknown'}`);
  else motionText(byId('source-status'), tr('noData'));
  motionText(byId('sync-status'), snapshot.observed_at_ms ? `${tr('observed')} ${date(snapshot.observed_at_ms)}` : tr('waitingFirst'));
}

function applyLanguage() {
  document.documentElement.lang = settings.language;
  document.querySelectorAll<HTMLElement>('[data-i18n]').forEach(element => {
    const key = element.dataset.i18n as MessageKey;
    if (key && messages.en[key]) element.textContent = tr(key);
  });
  byId('back').textContent = tr('back');
  byId('defaults').textContent = tr('defaults');
  byId('menu-refresh').textContent = tr('menuRefresh');
  byId('menu-settings').textContent = tr('menuSettings');
  byId('menu-quit').textContent = tr('quit');
  const pinLabel = settings.always_on_top ? tr('unpin') : tr('pin');
  byId('pin').setAttribute('aria-label', pinLabel);
  byId('pin').setAttribute('title', pinLabel);
  byId<HTMLInputElement>('path').placeholder = settings.language === 'zh-CN' ? '自动检测 ~/.codex/sessions' : 'Auto detect ~/.codex/sessions';
  const themeLabels = settings.language === 'zh-CN' ? ['跟随系统', '深色', '浅色', '毛玻璃（跟随系统）'] : ['System', 'Dark', 'Light', 'Glass (system)'];
  Array.from(byId<HTMLSelectElement>('theme').options).forEach((option, index) => { option.textContent = themeLabels[index]; });
  if (!refreshing) byId('refresh').textContent = tr('refresh');
  applySnapshot(snapshot);
}

function applySettings() {
  document.documentElement.dataset.theme = settings.theme;
  document.documentElement.dataset.animations = settings.animations_enabled ? 'on' : 'off';
  document.documentElement.style.setProperty('--background-opacity', String(settings.background_opacity));
  document.documentElement.style.setProperty('--text-opacity', String(settings.text_opacity));
  byId<HTMLInputElement>('background-opacity').value = String(Math.round(settings.background_opacity * 100));
  byId('background-opacity-value').textContent = `${Math.round(settings.background_opacity * 100)}%`;
  byId<HTMLInputElement>('text-opacity').value = String(Math.round(settings.text_opacity * 100));
  byId('text-opacity-value').textContent = `${Math.round(settings.text_opacity * 100)}%`;
  byId<HTMLSelectElement>('theme').value = settings.theme;
  byId<HTMLSelectElement>('language').value = settings.language;
  byId<HTMLInputElement>('animations').checked = settings.animations_enabled;
  byId('pin').classList.toggle('active', settings.always_on_top);
  byId('pin').setAttribute('aria-pressed', String(settings.always_on_top));
  byId<HTMLInputElement>('autostart').checked = settings.start_with_windows;
  byId<HTMLInputElement>('start-min').checked = settings.start_minimized;
  byId<HTMLInputElement>('close-tray').checked = settings.close_to_tray;
  byId<HTMLSelectElement>('interval').value = String(settings.refresh_interval_seconds);
  byId<HTMLInputElement>('path').value = settings.custom_session_path ?? '';
  applyLanguage();
}

async function showView(view: 'home' | 'settings') {
  const outgoing = byId<HTMLElement>(view === 'home' ? 'settings-view' : 'home-view');
  const incoming = byId<HTMLElement>(view === 'home' ? 'home-view' : 'settings-view');
  await closeMenu();
  if (!outgoing.hidden && !reducedMotion()) await animate(outgoing, { opacity: [1, 0], x: [0, view === 'home' ? 18 : -18], scale: [1, 0.985] }, { duration: 0.18, ease: 'easeIn' });
  outgoing.hidden = true;
  incoming.hidden = false;
  if (!reducedMotion()) {
    animate(incoming, { opacity: [0, 1], x: [view === 'home' ? -18 : 18, 0], scale: [0.985, 1] }, { duration: 0.38, ease: easeOut });
    enterElements(view === 'home' ? '#home-view header > *, #home-view .card, #home-view footer > *' : '#settings-view header > *, .settings-panel > *', 10);
  }
  if (view === 'settings') void loadDiagnostics();
}

function setRefreshing(value: boolean) {
  refreshing = value;
  const button = byId<HTMLButtonElement>('refresh');
  button.disabled = value;
  button.textContent = value ? tr('refreshing') : tr('refresh');
  button.classList.toggle('is-refreshing', value);
  if (!reducedMotion()) animate(button, value ? { scale: [1, 0.94, 1] } : { scale: [0.96, 1] }, spring);
}

async function refresh() {
  if (refreshing) return;
  setRefreshing(true);
  try { applySnapshot(await invoke<Snapshot>('refresh_quota')); }
  catch (error) { console.error('refresh failed', error); setRefreshing(false); }
  window.setTimeout(() => setRefreshing(false), 9000);
}

async function persistSettings() {
  settings = {
    window_opacity: 1,
    background_opacity: Number(byId<HTMLInputElement>('background-opacity').value) / 100,
    text_opacity: Number(byId<HTMLInputElement>('text-opacity').value) / 100,
    animations_enabled: byId<HTMLInputElement>('animations').checked,
    language: byId<HTMLSelectElement>('language').value as Settings['language'],
    always_on_top: settings.always_on_top,
    theme: byId<HTMLSelectElement>('theme').value,
    start_with_windows: byId<HTMLInputElement>('autostart').checked,
    start_minimized: byId<HTMLInputElement>('start-min').checked,
    close_to_tray: byId<HTMLInputElement>('close-tray').checked,
    refresh_interval_seconds: Number(byId<HTMLSelectElement>('interval').value),
    custom_session_path: byId<HTMLInputElement>('path').value.trim() || undefined,
  };
  try { settings = await invoke<Settings>('save_settings', { settings }); applySettings(); }
  catch (error) { console.error('save settings failed', error); }
}

async function loadDiagnostics(value?: Diagnostics) {
  try {
    const diagnostics = value ?? await invoke<Diagnostics>('get_diagnostics');
    const zh = settings.language === 'zh-CN';
    motionText(byId('diag-server'), `${zh ? '应用服务器' : 'App server'}: ${diagnostics.app_server_status || (zh ? '启动中' : 'starting')}`);
    motionText(byId('diag-executable'), `${zh ? 'Codex 可执行文件' : 'Codex executable'}: ${diagnostics.codex_executable || (zh ? '检查中' : 'checking')}`);
    motionText(byId('diag-process'), `${zh ? '已初始化' : 'Initialized'}: ${diagnostics.app_server_initialized ? (zh ? '是' : 'yes') : (zh ? '否' : 'no')}${diagnostics.app_server_pid ? ` · PID ${diagnostics.app_server_pid}` : ''}`);
    motionText(byId('diag-read'), `${zh ? '上次完整读取' : 'Last full read'}: ${diagnostics.app_server_last_read_ms ? date(diagnostics.app_server_last_read_ms) : (zh ? '从未' : 'never')}`);
    motionText(byId('diag-notification'), `${zh ? '上次通知' : 'Last notification'}: ${diagnostics.app_server_last_notification_ms ? date(diagnostics.app_server_last_notification_ms) : (zh ? '未观测到' : 'not observed')}`);
    motionText(byId('diag-bucket'), `${zh ? '配额桶' : 'Bucket'}: ${diagnostics.bucket ?? (zh ? '不可用' : 'unavailable')}`);
    motionText(byId('diag-watcher'), `${zh ? '会话监视器' : 'Session watcher'}: ${diagnostics.watcher_status || (zh ? '启动中' : 'starting')}`);
    motionText(byId('diag-source'), `${zh ? '当前来源' : 'Active source'}: ${sourceName(diagnostics.source)}`);
    motionText(byId('diag-error'), diagnostics.app_server_error ? `${zh ? '说明' : 'Note'}: ${diagnostics.app_server_error}` : '');
  } catch (error) { console.warn('diagnostics unavailable', error); }
}

byId('min').addEventListener('click', () => void getCurrentWindow().minimize());
byId('close').addEventListener('click', () => void getCurrentWindow().close());
byId('pin').addEventListener('click', async () => {
  const next = !settings.always_on_top;
  try {
    await getCurrentWindow().setAlwaysOnTop(next);
    settings.always_on_top = next;
    byId('pin').classList.toggle('active', next);
    byId('pin').setAttribute('aria-pressed', String(next));
    applyLanguage();
    if (!reducedMotion()) animate(byId('pin'), { rotate: [0, next ? -18 : 18, 0], scale: [1, 1.2, 1] }, spring);
    await persistSettings();
  } catch (error) { console.error('pin window failed', error); }
});
byId('more').addEventListener('click', event => { event.stopPropagation(); const menu = byId<HTMLElement>('menu'); menu.hidden ? openMenu() : void closeMenu(); });
byId('refresh').addEventListener('click', refresh);
byId('menu-refresh').addEventListener('click', refresh);
byId('menu-settings').addEventListener('click', () => void showView('settings'));
byId('menu-quit').addEventListener('click', () => void invoke('quit_app'));
byId('back').addEventListener('click', () => void showView('home'));
byId('titlebar').addEventListener('mousedown', event => { if (event.button === 0 && !(event.target as HTMLElement).closest('button,[data-no-drag]')) void getCurrentWindow().startDragging(); });
document.addEventListener('pointerdown', event => { const menu = byId<HTMLElement>('menu'); if (!menu.hidden && !menu.contains(event.target as Node) && !(event.target as HTMLElement).closest('#more')) void closeMenu(); });
document.addEventListener('keydown', event => { if (event.key === 'Escape') void showView('home'); });

for (const [id, property] of [['background-opacity', '--background-opacity'], ['text-opacity', '--text-opacity']] as const) {
  byId<HTMLInputElement>(id).addEventListener('input', event => {
    const value = Number((event.target as HTMLInputElement).value);
    motionText(byId(`${id}-value`), `${value}%`);
    document.documentElement.style.setProperty(property, String(value / 100));
  });
  byId(id).addEventListener('change', persistSettings);
}
for (const id of ['theme', 'language', 'animations', 'start-min', 'close-tray', 'interval', 'path']) byId(id).addEventListener('change', persistSettings);
byId('theme').addEventListener('change', () => { document.documentElement.dataset.theme = byId<HTMLSelectElement>('theme').value; });
byId('language').addEventListener('change', () => { settings.language = byId<HTMLSelectElement>('language').value as Settings['language']; applyLanguage(); });
byId('animations').addEventListener('change', () => { settings.animations_enabled = byId<HTMLInputElement>('animations').checked; document.documentElement.dataset.animations = settings.animations_enabled ? 'on' : 'off'; });
byId<HTMLInputElement>('autostart').addEventListener('change', async event => {
  const checked = (event.target as HTMLInputElement).checked;
  try { checked ? await enable() : await disable(); await persistSettings(); }
  catch (error) { console.error('autostart change failed', error); (event.target as HTMLInputElement).checked = !checked; }
});
byId('defaults').addEventListener('click', async () => {
  settings = { ...defaults };
  try { await disable(); } catch { /* the plugin may be unavailable outside packaged Tauri */ }
  applySettings();
  await persistSettings();
});

document.querySelectorAll<HTMLElement>('input, select').forEach(control => {
  control.addEventListener('change', () => {
    if (!reducedMotion()) animate(control, { scale: [0.97, 1.025, 1], filter: ['brightness(.9)', 'brightness(1.14)', 'brightness(1)'] }, spring);
  });
});

setInterval(() => {
  motionText(byId('five-count'), countdown(snapshot.five_hour?.resets_at_ms));
  motionText(byId('long-count'), countdown(snapshot.long_window?.resets_at_ms));
  updateSourceAge();
}, 1000);

function syncViewportScale() {
  const widthScale = Math.max(0.1, window.innerWidth / 520);
  const heightScale = Math.max(0.1, window.innerHeight / 620);
  const balanced = Math.sqrt(widthScale * heightScale);
  const scale = Math.max(0.1, Math.min(4, balanced, widthScale * 1.25, heightScale * 1.25));
  document.documentElement.style.setProperty('--ui-scale', scale.toFixed(4));
}

new ResizeObserver(syncViewportScale).observe(document.documentElement);
syncViewportScale();

async function bootstrap() {
  await listen<Snapshot>('quota-updated', event => { applySnapshot(event.payload); setRefreshing(false); });
  await listen<Diagnostics>('refresh-finished', event => { setRefreshing(false); void loadDiagnostics(event.payload); });
  await listen('settings-request', () => showView('settings'));
  try {
    settings = await invoke<Settings>('get_settings');
    try { settings.start_with_windows = await isEnabled(); } catch { /* keep persisted state */ }
    applySettings();
    await getCurrentWindow().setAlwaysOnTop(settings.always_on_top);
    applySnapshot(await invoke<Snapshot>('get_quota'));
    await loadDiagnostics();
    await refresh();
  } catch (error) { console.error('startup failed', error); }
  enterElements('.titlebar > *, #home-view header > *, #home-view .card, #home-view footer > *');
}

void bootstrap();
