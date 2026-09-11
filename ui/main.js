/* 剪贴板历史 —— 前端逻辑（纯静态，无构建步骤） */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// 自诊断：上报 UI 状态到 ui.log（排查问题用，出错不影响功能）
function uiReport(msg) {
  try { invoke('ui_report', { msg }); } catch (_) { /* ignore */ }
}
uiReport('script-start __TAURI__=' + typeof window.__TAURI__);
window.addEventListener('error', (e) => {
  uiReport('window.error: ' + e.message + ' @' + (e.filename || '') + ':' + (e.lineno || ''));
});
window.addEventListener('unhandledrejection', (e) => {
  uiReport('unhandledrejection: ' + ((e.reason && e.reason.message) || String(e.reason)));
});

const MAX_RENDER = 500;

const listEl = document.getElementById('list');
const countEl = document.getElementById('count');
const searchEl = document.getElementById('search');
const searchClearEl = document.getElementById('searchClear');
const toggleEl = document.getElementById('listeningToggle');
const statusDotEl = document.getElementById('statusDot');
const statusTextEl = document.getElementById('statusText');
const clearBtnEl = document.getElementById('clearBtn');
const copyAllBtnEl = document.getElementById('copyAllBtn');
const toastEl = document.getElementById('toast');

let entries = [];
let toastTimer = null;
let clearArmed = false;
let clearTimer = null;

/** 图标取自 index.html 中的 SVG 雪碧图（动物岛风格内置图标） */
function icon(id, cls) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('class', cls);
  svg.setAttribute('aria-hidden', 'true');
  const use = document.createElementNS('http://www.w3.org/2000/svg', 'use');
  use.setAttribute('href', '#' + id);
  svg.appendChild(use);
  return svg;
}

function fmtTime(ts) {
  if (!ts) return '';
  const d = new Date(ts * 1000);
  const now = new Date();
  const hm = d.toLocaleTimeString('zh-CN', {
    hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false,
  });
  if (d.toDateString() === now.toDateString()) return hm;
  return `${d.getMonth() + 1}月${d.getDate()}日 ${hm}`;
}

function setStatus(listening) {
  statusDotEl.classList.toggle('on', listening);
  statusTextEl.textContent = listening ? '监听中' : '已暂停';
  toggleEl.checked = listening;
}

function showToast(msg) {
  toastEl.textContent = '';
  toastEl.append(icon('ico-check', 'toast-ico'), document.createTextNode(msg));
  toastEl.hidden = false;
  toastEl.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    toastEl.classList.remove('show');
    setTimeout(() => { toastEl.hidden = true; }, 200);
  }, 1800);
}

function render(opts = {}) {
  const q = searchEl.value.trim().toLowerCase();
  const shown = q ? entries.filter((e) => e.text.toLowerCase().includes(q)) : entries;

  countEl.textContent = q
    ? `共 ${entries.length} 条，匹配 ${shown.length} 条`
    : `${entries.length} 条记录`;

  copyAllBtnEl.disabled = entries.length === 0;

  listEl.innerHTML = '';

  if (shown.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'empty';
    empty.append(
      icon('ico-file', 'empty-ico'),
      document.createTextNode(q
        ? '没有匹配的记录'
        : '暂无历史记录\n复制任意文本，就会自动记录在这里'),
    );
    listEl.appendChild(empty);
    return;
  }

  const frag = document.createDocumentFragment();
  const start = Math.max(0, shown.length - MAX_RENDER);
  for (let i = start; i < shown.length; i++) {
    const e = shown[i];
    const div = document.createElement('div');
    div.className = 'entry';
    div.title = '点击复制到剪贴板';

    const time = document.createElement('div');
    time.className = 'entry-time';
    time.textContent = fmtTime(e.timestamp);

    const text = document.createElement('div');
    text.className = 'entry-text';
    text.textContent = e.text;

    const hint = document.createElement('div');
    hint.className = 'entry-copy';
    hint.append(icon('ico-check', 'entry-copy-ico'), document.createTextNode('复制'));

    div.append(time, text, hint);
    div.addEventListener('click', () => copyEntry(e.text, div));
    frag.appendChild(div);
  }

  listEl.appendChild(frag);
  if (opts.scrollToBottom) listEl.scrollTop = listEl.scrollHeight;
}

async function copyEntry(text, el) {
  try {
    await invoke('copy_text', { text });
    el.classList.remove('copied');
    void el.offsetWidth; // 重启动画
    el.classList.add('copied');
    showToast('已复制到剪贴板');
  } catch (err) {
    showToast('复制失败：' + err);
  }
}

function nearBottom() {
  return listEl.scrollHeight - listEl.scrollTop - listEl.clientHeight < 48;
}

function disarmClear() {
  clearArmed = false;
  clearBtnEl.classList.remove('danger');
  clearBtnEl.textContent = '清空';
  clearTimeout(clearTimer);
}

(async function init() {
  try {
    const [hist, listening] = await Promise.all([
      invoke('get_history'),
      invoke('get_listening'),
    ]);
    entries = hist;
    setStatus(listening);
    render({ scrollToBottom: true });
    uiReport('init ok entries=' + hist.length + ' listening=' + listening);
  } catch (err) {
    uiReport('init failed: ' + err);
    showToast('初始化失败：' + err);
  }

  // 后端追加了新记录（或清空）时刷新列表；只有用户停在底部时才自动滚动
  try {
    await listen('history-updated', (ev) => {
      uiReport('history-updated received entries=' + ev.payload.length);
      const grew = ev.payload.length > entries.length;
      const shouldScroll = grew && !searchEl.value.trim() && nearBottom();
      entries = ev.payload;
      render({ scrollToBottom: shouldScroll });
    });
    uiReport('listener history-updated OK');
  } catch (err) {
    uiReport('listener history-updated FAILED: ' + err);
  }

  try {
    await listen('listening-changed', (ev) => setStatus(ev.payload));
    uiReport('listener listening-changed OK');
  } catch (err) {
    uiReport('listener listening-changed FAILED: ' + err);
  }

  // 剪贴板内容不是文本（图片/文件）时给出提示，避免用户以为监听失效
  try {
    await listen('clipboard-skipped', (ev) => {
      const reason = ev.payload;
      const msg = reason === 'image'
        ? '检测到图片内容，仅文本会被记录'
        : reason === 'files'
        ? '检测到文件内容，仅文本会被记录'
        : '剪贴板内容不是文本，已跳过';
      showToast(msg);
    });
    uiReport('listener clipboard-skipped OK');
  } catch (err) {
    uiReport('listener clipboard-skipped FAILED: ' + err);
  }

  toggleEl.addEventListener('change', async () => {
    try {
      await invoke('set_listening', { listening: toggleEl.checked });
    } catch (err) {
      toggleEl.checked = !toggleEl.checked;
      showToast('操作失败：' + err);
    }
  });

  copyAllBtnEl.addEventListener('click', async () => {
    if (entries.length === 0) return;
    try {
      await invoke('copy_all');
      uiReport('copy_all ok entries=' + entries.length);
      showToast('已复制全部 ' + entries.length + ' 条记录');
    } catch (err) {
      uiReport('copy_all failed: ' + err);
      showToast('复制失败：' + err);
    }
  });

  clearBtnEl.addEventListener('click', async () => {
    if (entries.length === 0) return;
    if (!clearArmed) {
      clearArmed = true;
      clearBtnEl.classList.add('danger');
      clearBtnEl.textContent = '确认清空？';
      clearTimer = setTimeout(disarmClear, 3000);
      return;
    }
    disarmClear();
    try {
      await invoke('clear_history');
      showToast('历史已清空');
    } catch (err) {
      showToast('清空失败：' + err);
    }
  });

  searchEl.addEventListener('input', () => {
    searchClearEl.hidden = !searchEl.value;
    render();
  });
  searchClearEl.addEventListener('click', () => {
    searchEl.value = '';
    searchClearEl.hidden = true;
    render();
  });

  window.addEventListener('keydown', (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
      e.preventDefault();
      searchEl.focus();
    } else if (e.key === 'Escape' && searchEl.value) {
      searchEl.value = '';
      searchClearEl.hidden = true;
      render();
    }
  });
})();
