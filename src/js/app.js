// Tauri IPC helpers
const { invoke, Channel } = window.__TAURI__.core;

let lastNotes = null;
let lastUpdateTimes = {};
let currentHistorySession = null;

// --- Navigation ---
document.querySelectorAll('.nav-btn').forEach(btn => {
  btn.addEventListener('click', () => {
    document.querySelectorAll('.nav-btn').forEach(b => b.classList.remove('active'));
    document.querySelectorAll('.view').forEach(v => v.classList.remove('active'));
    btn.classList.add('active');
    document.getElementById('view-' + btn.dataset.view).classList.add('active');

    if (btn.dataset.view === 'history') loadHistory();
    if (btn.dataset.view === 'profiles') loadProfiles();
    if (btn.dataset.view === 'settings') loadSettings();
  });
});

// --- Meeting Controls ---
async function updateTitle() {
  const title = document.getElementById('meetingTitle').value.trim();
  if (title) {
    try { await invoke('update_meeting_title', { title }); } catch (e) { /* ignore if no session */ }
  }
}

async function startMeeting() {
  const sources = getSelectedAudioSources();
  if (sources.length === 0) {
    // inline error without alert
    const btn = document.getElementById('audioSourceBtn');
    const orig = btn.style.outline;
    btn.style.outline = '2px solid var(--danger)';
    setTimeout(() => { btn.style.outline = orig; }, 2000);
    return;
  }

  const title = document.getElementById('meetingTitle').value || undefined;
  const profileId = document.getElementById('profileSelect').value || null;

  const hasSystem = sources.includes('system');
  const mics = sources.filter(v => v !== 'system');
  const micDevice = mics.length > 0 ? mics[0] : undefined;

  let audioSource;
  if (hasSystem && micDevice)  audioSource = 'both';
  else if (hasSystem)          audioSource = 'system';
  else                         audioSource = 'microphone';

  try {
    const onNotes = new Channel();
    onNotes.onmessage = (notes) => {
      console.log('Notes received via channel');
      renderNotes(notes);
    };
    const onState = new Channel();
    onState.onmessage = (state) => {
      const s = state.toLowerCase();
      setMeetingControls(s === 'completed' ? 'idle' : s);
    };
    await invoke('start_meeting', { audioSource, title, profileId, micDevice, onNotes, onState });
    // Clear previous meeting notes so the new session starts fresh.
    lastNotes = null;
    lastUpdateTimes = {};
    document.getElementById('notesContainer').innerHTML = '';
    setMeetingControls('active');
  } catch (e) {
    const errEl = document.getElementById('meetingError');
    errEl.textContent = 'Failed to start: ' + e;
    setTimeout(() => { errEl.textContent = ''; }, 5000);
  }
}

async function pauseMeeting() {
  try {
    await invoke('pause_meeting');
    setMeetingControls('paused');
  } catch (e) { showMeetingError(e); }
}

async function resumeMeeting() {
  try {
    await invoke('resume_meeting');
    setMeetingControls('active');
  } catch (e) { showMeetingError(e); }
}

async function stopMeeting() {
  try {
    setMeetingControls('idle');
    await invoke('stop_meeting');
  } catch (e) { console.error('Stop meeting failed:', e); }
}

function showMeetingError(e) {
  const errEl = document.getElementById('meetingError');
  errEl.textContent = String(e);
  setTimeout(() => { errEl.textContent = ''; }, 5000);
}

function setMeetingControls(state) {
  const start = document.getElementById('btnStart');
  const pause = document.getElementById('btnPause');
  const resume = document.getElementById('btnResume');
  const stop = document.getElementById('btnStop');
  const status = document.getElementById('navStatus');

  start.disabled = state !== 'idle';
  pause.disabled = state !== 'active';
  pause.style.display = state === 'paused' ? 'none' : '';
  resume.style.display = state === 'paused' ? '' : 'none';
  stop.disabled = state === 'idle';
  status.textContent = state.charAt(0).toUpperCase() + state.slice(1);
}

async function sendInstruction() {
  const input = document.getElementById('instructionInput');
  const text = input.value.trim();
  if (!text) return;
  try {
    await invoke('send_instruction', { text });
    input.value = '';
  } catch (e) { console.error(e); }
}

// --- Document Attachment ---
function showDocPaste() {
  document.getElementById('docEmpty').style.display = 'none';
  document.getElementById('docPaste').style.display = 'block';
}

function hideDocPaste() {
  document.getElementById('docPaste').style.display = 'none';
  document.getElementById('docEmpty').style.display = 'flex';
  document.getElementById('docPasteInput').value = '';
}

async function attachDocText() {
  const text = document.getElementById('docPasteInput').value.trim();
  if (!text) return;
  try {
    await invoke('attach_document_text', { text });
    showDocAttached('Pasted text', text.length);
  } catch (e) {
    setDocError(String(e));
  }
}

async function attachDocFile(input) {
  const file = input.files[0];
  if (!file) return;
  try {
    const buf = await file.arrayBuffer();
    const bytes = new Uint8Array(buf);
    const data = Array.from(bytes);
    const name = await invoke('attach_document_file', { filename: file.name, data });
    showDocAttached(name, file.size);
  } catch (e) {
    setDocError(String(e));
  }
  input.value = '';
}

function showDocAttached(name, size) {
  const sizeStr = size > 1024 ? `${(size / 1024).toFixed(1)} KB` : `${size} chars`;
  document.getElementById('docEmpty').style.display = 'none';
  document.getElementById('docPaste').style.display = 'none';
  const status = document.getElementById('docStatus');
  status.textContent = `✓ ${name} (${sizeStr})`;
  status.className = 'doc-status';
}

function setDocError(msg) {
  const status = document.getElementById('docStatus');
  status.textContent = msg;
  status.className = 'doc-status error';
}

async function copyNotes() {
  if (!lastNotes) return;
  try {
    const md = notesToMarkdown(lastNotes);
    await navigator.clipboard.writeText(md);
  } catch (e) { console.error('Copy failed:', e); }
}

// --- Live Notes Rendering ---
function renderNotes(notes) {
  lastNotes = notes;
  const c = document.getElementById('notesContainer');
  let html = '';

  if (notes.summary && notes.summary.content && notes.summary.block_state !== 'UserDeleted') {
    const updated = isRecent(notes.summary.last_updated_at, 'summary');
    const stateClass = blockStateClass(notes.summary);
    html += `<div class="section ${updated ? 'updated' : ''} ${stateClass}" data-block-id="${esc(notes.summary.id)}">
      <h2>Summary</h2>
      <div class="block-wrapper">
        <div class="content" contenteditable="true" data-block-id="${esc(notes.summary.id)}" data-section="summary" onblur="handleBlockEdit(this)">${md(notes.summary.content)}</div>
        <button class="block-delete" title="Delete" onclick="deleteBlock('${esc(notes.summary.id)}')">×</button>
      </div>
    </div>`;
  }

  if (notes.action_items && notes.action_items.length > 0) {
    const visible = notes.action_items.filter(a => a.block_state !== 'UserDeleted');
    if (visible.length > 0) {
      const updated = visible.some(a => isRecent(a.last_updated_at, 'ai-' + a.id));
      let items = visible.map(a => {
        const stateClass = blockStateClass(a);
        return `<div class="action-item ${stateClass}" data-block-id="${esc(a.id)}">
          <span class="assignee">@${esc(a.assignee || 'Unassigned')}</span>
          <span class="block-text" contenteditable="true" data-block-id="${esc(a.id)}" data-section="action_items" onblur="handleBlockEdit(this)">${esc(a.description)}</span>
          ${stateClass ? '<span class="edit-badge">✎</span>' : ''}
          <button class="block-delete" title="Delete" onclick="deleteBlock('${esc(a.id)}')">×</button>
        </div>`;
      }).join('');
      html += `<div class="section ${updated ? 'updated' : ''}"><h2>Action Items</h2>${items}<button class="btn btn-add" onclick="addBlock('action_items', this)">+ Add Action Item</button></div>`;
    }
  }

  if (notes.decisions && notes.decisions.length > 0) {
    const visible = notes.decisions.filter(d => d.block_state !== 'UserDeleted');
    if (visible.length > 0) {
      const updated = visible.some(d => isRecent(d.last_updated_at, 'dec-' + d.id));
      let decs = visible.map(d => {
        const stateClass = blockStateClass(d);
        return `<div class="decision ${stateClass}" data-block-id="${esc(d.id)}">
          <span class="block-text" contenteditable="true" data-block-id="${esc(d.id)}" data-section="decisions" onblur="handleBlockEdit(this)">${esc(d.decision_text)}</span>
          ${stateClass ? '<span class="edit-badge">✎</span>' : ''}
          <button class="block-delete" title="Delete" onclick="deleteBlock('${esc(d.id)}')">×</button>
        </div>`;
      }).join('');
      html += `<div class="section ${updated ? 'updated' : ''}"><h2>Decisions</h2>${decs}<button class="btn btn-add" onclick="addBlock('decisions', this)">+ Add Decision</button></div>`;
    }
  }

  if (notes.discussion_topics && notes.discussion_topics.length > 0) {
    const visible = notes.discussion_topics.filter(t => t.block_state !== 'UserDeleted');
    if (visible.length > 0) {
      const updated = visible.some(t => isRecent(t.last_updated_at, 'topic-' + t.id));
      let topics = visible.map(t => {
        const stateClass = blockStateClass(t);
        return `<div class="topic ${stateClass}" data-block-id="${esc(t.id)}">
          <h3 contenteditable="true" data-block-id="${esc(t.id)}" data-field="title" onblur="handleTopicTitleEdit(this)">${esc(t.topic_title)}</h3>
          <div class="content" contenteditable="true" data-block-id="${esc(t.id)}" data-section="discussion_topics" onblur="handleBlockEdit(this)">${md(t.content || '')}</div>
          ${stateClass ? '<span class="edit-badge">✎</span>' : ''}
          <button class="block-delete" title="Delete" onclick="deleteBlock('${esc(t.id)}')">×</button>
        </div>`;
      }).join('');
      html += `<div class="section ${updated ? 'updated' : ''}"><h2>Discussion Topics</h2>${topics}<button class="btn btn-add" onclick="addBlock('discussion_topics', this)">+ Add Topic</button></div>`;
    }
  }

  if (!html) html = '<div class="section"><div class="empty">Waiting for meeting notes...</div></div>';
  c.innerHTML = html;
  renderDeletedItems(notes);
  // Show corrections tray when we have notes
  document.getElementById('correctionsTray').style.display = 'block';
}

function blockStateClass(block) {
  if (block.block_state === 'UserEdited') return 'user-edited';
  if (block.block_state === 'UserAdded') return 'user-added';
  return '';
}

async function handleBlockEdit(el) {
  const blockId = el.dataset.blockId;
  const content = el.innerText.trim();
  // Compare against what was there when we started editing
  if (content === (el._originalText || '')) return;
  try {
    await invoke('edit_note_block', { blockId, content });
    const wrapper = el.closest('[data-block-id]');
    if (wrapper) wrapper.classList.add('user-edited');
  } catch (e) { console.error('Edit failed:', e); }
}

async function handleTopicTitleEdit(el) {
  const blockId = el.dataset.blockId;
  const newTitle = el.innerText.trim();
  if (newTitle === (el._originalText || '')) return;
  const topicEl = el.closest('.topic');
  const contentEl = topicEl?.querySelector('.content[contenteditable]');
  const body = contentEl ? contentEl.innerText.trim() : '';
  const combined = newTitle + '\n' + body;
  try {
    await invoke('edit_note_block', { blockId, content: combined });
    const wrapper = el.closest('[data-block-id]');
    if (wrapper) wrapper.classList.add('user-edited');
  } catch (e) { console.error('Title edit failed:', e); }
}

function getOriginalContent(blockId) {
  if (!lastNotes) return '';
  if (lastNotes.summary?.id === blockId) return lastNotes.summary.content;
  const ai = lastNotes.action_items?.find(a => a.id === blockId);
  if (ai) return ai.description;
  const d = lastNotes.decisions?.find(d => d.id === blockId);
  if (d) return d.decision_text;
  const t = lastNotes.discussion_topics?.find(t => t.id === blockId);
  if (t) return t.content || '';
  return '';
}

async function deleteBlock(blockId) {
  try {
    await invoke('delete_note_block', { blockId });
    // Remove from DOM immediately
    document.querySelectorAll(`[data-block-id="${blockId}"]`).forEach(el => {
      const section = el.closest('.section');
      el.remove();
      if (section && section.dataset.blockId === blockId) section.remove();
    });
  } catch (e) { console.error('Delete failed:', e); }
}

function addBlock(section, btn) {
  // Find the add button's parent section and insert an empty editable block before the button
  const sectionEl = btn.closest('.section');
  if (!sectionEl) return;

  const placeholder = section === 'discussion_topics'
    ? `<div class="topic user-added new-block" data-section="${section}">
         <h3 contenteditable="true" class="new-title" placeholder="Topic title..." data-section="${section}"></h3>
         <div class="content" contenteditable="true" class="new-content" placeholder="Content..." data-section="${section}"></div>
       </div>`
    : `<div class="${section === 'decisions' ? 'decision' : 'action-item'} user-added new-block" data-section="${section}">
         <span class="block-text" contenteditable="true" placeholder="${section === 'decisions' ? 'Decision...' : 'Action item...'}"></span>
       </div>`;

  btn.insertAdjacentHTML('beforebegin', placeholder);
  const newEl = btn.previousElementSibling;
  const editable = newEl.querySelector('[contenteditable]');
  editable.focus();

  // Save on blur
  newEl.addEventListener('focusout', async () => {
    // Wait a tick — if focus moved to another editable within the same new-block, don't save yet
    await new Promise(r => setTimeout(r, 50));
    if (newEl.contains(document.activeElement)) return;
    if (!newEl.classList.contains('new-block')) return; // already saved
    newEl.classList.remove('new-block');

    let content;
    if (section === 'discussion_topics') {
      const title = newEl.querySelector('h3').innerText.trim();
      content = newEl.querySelector('.content').innerText.trim();
      if (!title && !content) { newEl.remove(); return; }
      content = (title || 'New Topic') + '\n' + content;
    } else {
      content = newEl.querySelector('[contenteditable]').innerText.trim();
      if (!content) { newEl.remove(); return; }
    }

    try {
      const blockId = await invoke('add_note_block', { section, content });
      newEl.dataset.blockId = blockId;
      // Update local notes and re-render to get proper structure
      if (lastNotes) {
        const now = Date.now();
        const base = { id: blockId, content, last_updated_by: 'User', last_updated_at: now, block_state: 'UserAdded', original_ai_content: null };
        if (section === 'action_items') {
          lastNotes.action_items = lastNotes.action_items || [];
          lastNotes.action_items.push({ ...base, description: content, assignee: null });
        } else if (section === 'decisions') {
          lastNotes.decisions = lastNotes.decisions || [];
          lastNotes.decisions.push({ ...base, decision_text: content });
        } else if (section === 'discussion_topics') {
          const title = newEl.querySelector('h3').innerText.trim() || 'New Topic';
          const body = newEl.querySelector('.content').innerText.trim();
          lastNotes.discussion_topics = lastNotes.discussion_topics || [];
          lastNotes.discussion_topics.push({ ...base, topic_title: title, content: body });
        }
        renderNotes(lastNotes);
      }
    } catch (e) { console.error('Add failed:', e); newEl.remove(); }
  });
}

// --- Trays: Deleted Items & Corrections ---
function toggleTray(id) {
  const tray = document.getElementById(id);
  const body = tray.querySelector('.tray-body');
  const toggle = tray.querySelector('.tray-toggle');
  const open = body.style.display === 'none';
  body.style.display = open ? 'block' : 'none';
  toggle.textContent = open ? '▾' : '▸';
  if (open && id === 'correctionsTray') loadCorrections();
}

function renderDeletedItems(notes) {
  const tray = document.getElementById('deletedTray');
  const body = document.getElementById('deletedTrayBody');
  const deleted = [];
  if (notes.summary?.block_state === 'UserDeleted') deleted.push({ id: notes.summary.id, label: 'Summary', text: notes.summary.content });
  (notes.discussion_topics || []).filter(t => t.block_state === 'UserDeleted').forEach(t => deleted.push({ id: t.id, label: 'Topic', text: t.topic_title }));
  (notes.decisions || []).filter(d => d.block_state === 'UserDeleted').forEach(d => deleted.push({ id: d.id, label: 'Decision', text: d.decision_text }));
  (notes.action_items || []).filter(a => a.block_state === 'UserDeleted').forEach(a => deleted.push({ id: a.id, label: 'Action Item', text: a.description }));

  if (deleted.length === 0) { tray.style.display = 'none'; return; }
  tray.style.display = 'block';
  body.innerHTML = deleted.map(d =>
    `<div class="tray-item"><span class="tray-label">${esc(d.label)}</span> <span class="tray-text">${esc(d.text)}</span> <button class="btn btn-sm" onclick="restoreBlock('${esc(d.id)}')">Restore</button></div>`
  ).join('');
}

async function restoreBlock(blockId) {
  try {
    await invoke('restore_note_block', { blockId });
    // Update local notes and re-render
    if (lastNotes) {
      // Find in flattened sections and reset state
      [lastNotes.summary].forEach(b => { if (b.id === blockId) b.block_state = 'AiManaged'; });
      (lastNotes.discussion_topics||[]).forEach(t => { if (t.id === blockId) t.block_state = 'AiManaged'; });
      (lastNotes.decisions||[]).forEach(d => { if (d.id === blockId) d.block_state = 'AiManaged'; });
      (lastNotes.action_items||[]).forEach(a => { if (a.id === blockId) a.block_state = 'AiManaged'; });
      renderNotes(lastNotes);
    }
  } catch (e) { console.error('Restore failed:', e); }
}

async function loadCorrections() {
  const body = document.getElementById('correctionsTrayBody');
  try {
    const corrections = await invoke('get_corrections');
    if (corrections.length === 0) {
      body.innerHTML = '<div class="tray-empty">No corrections yet.</div>';
      return;
    }
    body.innerHTML = corrections.map((c, i) =>
      `<div class="tray-item"><span class="tray-text">"${esc(c.original)}" → "${esc(c.corrected)}"</span> <button class="btn btn-sm" onclick="removeCorrection(${i})">×</button></div>`
    ).join('');
  } catch (e) { body.innerHTML = '<div class="tray-empty">No active session.</div>'; }
}

async function removeCorrection(index) {
  try {
    await invoke('remove_correction', { index });
    loadCorrections();
  } catch (e) { console.error('Remove correction failed:', e); }
}

function isRecent(ts, key) {
  const prev = lastUpdateTimes[key];
  lastUpdateTimes[key] = ts;
  return prev !== undefined && prev !== ts;
}

function esc(s) { const d = document.createElement('div'); d.textContent = s; return d.innerHTML; }

function md(s) {
  if (!s) return '';
  return esc(s)
    .replace(/^- (.+)$/gm, '<li>$1</li>')
    .replace(/(<li>.*<\/li>)/s, '<ul>$1</ul>')
    .replace(/\*\*(.+?)\*\*/g, '<b>$1</b>')
    .replace(/\n(?!<)/g, '<br>');
}

function notesToMarkdown(notes) {
  let out = `# ${notes.title || 'Meeting'}\n\n`;
  if (notes.summary?.content && notes.summary.block_state !== 'UserDeleted') out += `## Summary\n${notes.summary.content}\n\n`;
  const topics = (notes.discussion_topics || []).filter(t => t.block_state !== 'UserDeleted');
  if (topics.length) {
    out += '## Discussion Topics\n';
    topics.forEach(t => { out += `### ${t.topic_title}\n${t.content || ''}\n\n`; });
  }
  const decs = (notes.decisions || []).filter(d => d.block_state !== 'UserDeleted');
  if (decs.length) {
    out += '## Decisions\n';
    decs.forEach(d => { out += `- ${d.decision_text}\n`; });
    out += '\n';
  }
  const actions = (notes.action_items || []).filter(a => a.block_state !== 'UserDeleted');
  if (actions.length) {
    out += '## Action Items\n';
    actions.forEach(a => { out += `- [ ] @${a.assignee || 'Unassigned'}: ${a.description}\n`; });
  }
  return out;
}

// --- History ---
async function loadHistory() {
  try {
    const sessions = await invoke('list_sessions');
    const list = document.getElementById('historyList');
    if (!sessions.length) {
      list.innerHTML = '<div class="empty">No past sessions.</div>';
      return;
    }
    sessions.sort((a, b) => b.start_time - a.start_time);
    list.innerHTML = sessions.map(s => {
      const date = new Date(s.start_time).toLocaleDateString();
      return `<div class="history-item">
        <div onclick="viewSession('${s.session_id}')" style="flex:1;cursor:pointer"><span class="title">${esc(s.title)}</span><br><span class="meta">${date} · ${s.state}</span></div>
        <button class="btn btn-sm btn-danger" onclick="deleteSession(event, '${s.session_id}')">Delete</button>
      </div>`;
    }).join('');
  } catch (e) { console.error(e); }
}

function renderHistoryNotes(notes) {
  let html = '';
  if (notes.summary?.content) html += `<div class="section"><h2>Summary</h2><div class="content">${md(notes.summary.content)}</div></div>`;
  if (notes.action_items?.length) {
    html += '<div class="section"><h2>Action Items</h2>';
    notes.action_items.forEach(a => { html += `<div class="action-item"><span class="assignee">@${esc(a.assignee || 'Unassigned')}</span><span>${esc(a.description)}</span></div>`; });
    html += '</div>';
  }
  if (notes.decisions?.length) {
    html += '<div class="section"><h2>Decisions</h2>';
    notes.decisions.forEach(d => { html += `<div class="decision">${esc(d.decision_text)}</div>`; });
    html += '</div>';
  }
  if (notes.discussion_topics?.length) {
    html += '<div class="section"><h2>Discussion Topics</h2>';
    notes.discussion_topics.forEach(t => { html += `<div class="topic"><h3>${esc(t.topic_title)}</h3><div class="content">${md(t.content || '')}</div></div>`; });
    html += '</div>';
  }
  document.getElementById('historyNotes').innerHTML = html || '<div class="empty">No notes.</div>';
}

async function viewSession(id) {
  try {
    currentHistorySession = id;
    const session = await invoke('get_session', { sessionId: id });
    document.getElementById('historyList').style.display = 'none';
    const detail = document.getElementById('historyDetail');
    detail.style.display = 'block';
    // Reset answer area
    document.getElementById('historyAnswer').style.display = 'none';
    document.getElementById('historyInstructionInput').value = '';
    // Render notes in the detail view
    renderHistoryNotes(session.notes);
    // Render transcript
    const transcriptEl = document.getElementById('historyTranscript');
    const hasTranscript = !!(session.transcript?.length);
    if (hasTranscript) {
      const lines = session.transcript.map(s => {
        const speaker = s.speaker || 'Unknown';
        const mins = Math.floor(s.start_time_ms / 60000);
        const secs = Math.floor((s.start_time_ms / 1000) % 60);
        const ts = `${String(mins).padStart(2,'0')}:${String(secs).padStart(2,'0')}`;
        return `<div class="transcript-line"><span class="ts">${ts}</span> <span class="speaker">${esc(speaker)}</span>: ${esc(s.text)}</div>`;
      }).join('');
      document.getElementById('historyTranscriptContent').innerHTML = lines;
      transcriptEl.style.display = 'none'; // hidden by default, toggle to show
    } else {
      transcriptEl.style.display = 'none';
    }
    // Show/hide Toggle Transcript button based on whether transcript data exists
    document.getElementById('toggleTranscriptBtn').style.display = hasTranscript ? '' : 'none';
  } catch (e) { console.error('viewSession error:', e); }
}

function toggleTranscript() {
  const el = document.getElementById('historyTranscript');
  el.style.display = el.style.display === 'none' ? 'block' : 'none';
}

async function querySession() {
  if (!currentHistorySession) return;
  const input = document.getElementById('historyInstructionInput');
  const question = input.value.trim();
  if (!question) return;
  const answerEl = document.getElementById('historyAnswer');
  const contentEl = document.getElementById('historyAnswerContent');
  answerEl.style.display = 'block';
  contentEl.innerHTML = '<em>Thinking...</em>';
  try {
    const notes = await invoke('query_session', { sessionId: currentHistorySession, question });
    contentEl.innerHTML = '<em>Notes updated — scroll up to see the changes.</em>';
    input.value = '';
    renderHistoryNotes(notes);
  } catch (e) {
    contentEl.innerHTML = `<span style="color:var(--danger)">${esc(String(e))}</span>`;
  }
}

function closeHistoryDetail() {
  document.getElementById('historyDetail').style.display = 'none';
  document.getElementById('historyList').style.display = '';
  currentHistorySession = null;
}

async function deleteSession(event, id) {
  event.stopPropagation();
  if (!confirm('Delete this session? This cannot be undone.')) return;
  try {
    await invoke('delete_session', { sessionId: id });
    loadHistory();
  } catch (e) { console.error('Delete failed:', e); }
}

async function saveSessionFile(format) {
  if (!currentHistorySession) return;
  try {
    const path = await invoke('save_session_file', { sessionId: currentHistorySession, format });
    showExportStatus(`Saved to ${path}`);
  } catch (e) { showExportStatus(`Error: ${e}`, true); }
}

async function copySessionToClipboard(format) {
  if (!currentHistorySession) return;
  try {
    const text = await invoke('export_session', { sessionId: currentHistorySession, format });
    await navigator.clipboard.writeText(text);
    showExportStatus('Copied to clipboard');
  } catch (e) { showExportStatus(`Error: ${e}`, true); }
}

function showExportStatus(msg, isError = false) {
  let el = document.getElementById('exportStatus');
  if (!el) {
    el = document.createElement('div');
    el.id = 'exportStatus';
    document.querySelector('.history-actions').insertAdjacentElement('afterend', el);
  }
  el.textContent = msg;
  el.className = 'export-status' + (isError ? ' error' : '');
  setTimeout(() => { el.textContent = ''; }, 4000);
}

// --- Profiles ---
async function loadProfiles() {
  try {
    const profiles = await invoke('list_profiles');
    const list = document.getElementById('profileList');
    list.innerHTML = profiles.map(p => `
      <div class="profile-item">
        <div>
          <span class="name">${esc(p.name)}</span>
          <br><span style="color:var(--muted);font-size:0.8rem">${esc(p.instructions || 'No instructions').substring(0, 80)}</span>
        </div>
        <div>
          ${p.id !== 'default' ? `<button class="btn" onclick="editProfile('${p.id}')">Edit</button>
          <button class="btn btn-danger" onclick="deleteProfileById('${p.id}')">Delete</button>` : '<span style="color:var(--muted)">Built-in</span>'}
        </div>
      </div>
    `).join('');
  } catch (e) { console.error(e); }
}

function showProfileForm(profile) {
  document.getElementById('profileForm').style.display = 'block';
  document.getElementById('profileFormTitle').textContent = profile ? 'Edit Profile' : 'New Profile';
  document.getElementById('profileId').value = profile?.id || '';
  document.getElementById('profileName').value = profile?.name || '';
  document.getElementById('profileInstructions').value = profile?.instructions || '';
}

function hideProfileForm() {
  document.getElementById('profileForm').style.display = 'none';
}

async function editProfile(id) {
  try {
    const profile = await invoke('get_profile', { id });
    showProfileForm(profile);
  } catch (e) { console.error('Failed to load profile:', e); }
}

async function saveProfileForm() {
  const errEl = document.getElementById('profileFormError');
  errEl.textContent = '';
  const id = document.getElementById('profileId').value || crypto.randomUUID();
  const name = document.getElementById('profileName').value.trim();
  if (!name) { errEl.textContent = 'Name is required'; return; }
  const profile = {
    id,
    name,
    sections: [
      { section_type: 'Summary', enabled: true, custom_name: null },
      { section_type: 'DiscussionTopics', enabled: true, custom_name: null },
      { section_type: 'Decisions', enabled: true, custom_name: null },
      { section_type: 'ActionItems', enabled: true, custom_name: null },
    ],
    instructions: document.getElementById('profileInstructions').value,
  };
  try {
    await invoke('save_profile', { profile });
    hideProfileForm();
    loadProfiles();
  } catch (e) { errEl.textContent = String(e); }
}

async function deleteProfileById(id) {
  if (!confirm('Delete this profile?')) return;
  try {
    await invoke('delete_profile', { id });
    loadProfiles();
  } catch (e) { console.error('Delete failed:', e); }
}

// --- Whisper Model Download ---
async function downloadWhisperModel(modelId) {
  const btn = document.querySelector(`[data-model-id="${modelId}"] button`);
  const statusEl = document.getElementById(`modelStatus-${modelId}`);
  if (!btn || !statusEl) return;

  btn.disabled = true;
  statusEl.textContent = 'Starting...';
  statusEl.className = 'model-status downloading';

  try {
    const { Channel } = window.__TAURI__.core;
    const onProgress = new Channel();
    onProgress.onmessage = (fraction) => {
      if (fraction >= 0) {
        const pct = Math.round(fraction * 100);
        statusEl.textContent = `${pct}%`;
      }
    };

    const path = await invoke('download_whisper_model', { modelId, onProgress });

    // Auto-fill the model path field and mark as downloaded
    document.getElementById('settingsWhisperModelPath').value = path;
    statusEl.textContent = 'Downloaded';
    statusEl.className = 'model-status downloaded';
    btn.textContent = 'Re-download';
    btn.disabled = false;
  } catch (e) {
    statusEl.textContent = 'Failed';
    statusEl.className = 'model-status error';
    btn.disabled = false;
    console.error('Download failed:', e);
  }
}

// --- Settings ---
function toggleAiProviderFields() {
  const provider = document.getElementById('settingsAiProvider').value;
  document.getElementById('claudeFields').style.display = provider === 'claude' ? '' : 'none';
  document.getElementById('claudeCliFields').style.display = provider === 'claude-cli' ? '' : 'none';
  document.getElementById('openaiFields').style.display = provider === 'openai' ? '' : 'none';
}

function toggleTranscriptionFields() {
  const provider = document.getElementById('settingsTranscriptionProvider').value;
  document.getElementById('awsTranscribeFields').style.display = provider === 'aws' ? '' : 'none';
  document.getElementById('whisperFields').style.display = provider === 'whisper' ? '' : 'none';
}

async function loadSettings() {
  try {
    const config = await invoke('get_config');
    const provider = config.ai_provider || 'claude';
    document.getElementById('settingsAiProvider').value = provider;
    document.getElementById('settingsClaudeApiKey').value = config.claude_api_key || '';
    document.getElementById('settingsClaudeModel').value = config.claude_model || '';
    document.getElementById('settingsClaudeCliPath').value = config.claude_cli_path || '';
    document.getElementById('settingsOpenAiKey').value = config.openai_api_key || '';
    document.getElementById('settingsOpenAiModel').value = config.openai_model || '';
    const txProvider = config.transcription_provider || 'aws';
    document.getElementById('settingsTranscriptionProvider').value = txProvider;
    const whisperPath = config.whisper_model_path || '';
    document.getElementById('settingsWhisperModelPath').value = whisperPath;
    document.getElementById('settingsAwsProfile').value = config.aws_profile || '';
    document.getElementById('settingsAwsRegion').value = config.aws_region || '';
    document.getElementById('settingsVerbose').checked = config.verbose_logging || false;
    toggleAiProviderFields();
    toggleTranscriptionFields();

    // Mark any already-downloaded model in the picker
    const modelIds = ['tiny.en', 'base.en', 'small.en', 'medium.en'];
    modelIds.forEach(id => {
      const statusEl = document.getElementById(`modelStatus-${id}`);
      const btn = document.querySelector(`[data-model-id="${id}"] button`);
      if (!statusEl || !btn) return;
      if (whisperPath && whisperPath.includes(`ggml-${id}`)) {
        statusEl.textContent = 'Downloaded';
        statusEl.className = 'model-status downloaded';
        btn.textContent = 'Re-download';
      } else {
        statusEl.textContent = '';
        statusEl.className = 'model-status';
        btn.textContent = 'Download';
      }
    });

  } catch (e) { console.error(e); }
}

async function saveSettings() {
  const config = {
    ai_provider: document.getElementById('settingsAiProvider').value || 'claude',
    claude_api_key: document.getElementById('settingsClaudeApiKey').value || null,
    claude_model: document.getElementById('settingsClaudeModel').value || null,
    claude_cli_path: document.getElementById('settingsClaudeCliPath').value || null,
    openai_api_key: document.getElementById('settingsOpenAiKey').value || null,
    openai_model: document.getElementById('settingsOpenAiModel').value || null,
    transcription_provider: document.getElementById('settingsTranscriptionProvider').value || 'aws',
    whisper_model_path: document.getElementById('settingsWhisperModelPath').value || null,
    aws_profile: document.getElementById('settingsAwsProfile').value || null,
    aws_region: document.getElementById('settingsAwsRegion').value || null,
    verbose_logging: document.getElementById('settingsVerbose').checked,
  };
  const statusEl = document.getElementById('settingsSaveStatus');
  statusEl.textContent = '';
  statusEl.className = 'save-status';
  try {
    await invoke('save_config', { config });
    statusEl.textContent = 'Settings saved';
    statusEl.className = 'save-status success';
    setTimeout(() => { statusEl.textContent = ''; statusEl.className = 'save-status'; }, 3000);
  } catch (e) {
    statusEl.textContent = String(e);
    statusEl.className = 'save-status error';
  }
}

// --- Audio Source Selector ---

function buildAudioItem(value, displayName) {
  const label = document.createElement('label');
  label.className = 'audio-source-item';
  const cb = document.createElement('input');
  cb.type = 'checkbox';
  cb.value = value;
  cb.dataset.audioSource = '';
  cb.addEventListener('change', updateAudioSourceLabel);
  const span = document.createElement('span');
  span.textContent = displayName;
  label.appendChild(cb);
  label.appendChild(span);
  return label;
}

async function populateAudioSources() {
  const panel = document.getElementById('audioSourcePanel');
  const prev = getSelectedAudioSources();

  // Clear existing items
  panel.innerHTML = '';

  // System Audio — always first
  panel.appendChild(buildAudioItem('system', 'System Audio'));

  // Mic devices
  let devices = [];
  try {
    devices = await invoke('list_audio_devices');
  } catch (e) { console.error('Failed to list audio devices:', e); }

  devices.forEach(([name]) => {
    panel.appendChild(buildAudioItem(name, name));
  });

  // Restore selection or default to system audio
  const allValues = ['system', ...devices.map(([name]) => name)];
  const toCheck = prev.length ? prev.filter(v => allValues.includes(v)) : ['system'];
  panel.querySelectorAll('[data-audio-source]').forEach(cb => {
    cb.checked = toCheck.includes(cb.value);
  });

  updateAudioSourceLabel();
}

function getSelectedAudioSources() {
  return [...document.querySelectorAll('[data-audio-source]:checked')].map(cb => cb.value);
}

function updateAudioSourceLabel() {
  const selected = getSelectedAudioSources();
  const labelEl = document.getElementById('audioSourceLabel');
  if (selected.length === 0) {
    labelEl.textContent = 'No sources';
  } else {
    // Build display names
    const names = selected.map(val => {
      if (val === 'system') return 'System Audio';
      const cb = document.querySelector(`[data-audio-source][value="${CSS.escape(val)}"]`);
      return cb ? cb.closest('label').querySelector('span').textContent : val;
    });
    if (names.length <= 2) {
      labelEl.textContent = names.join(' + ');
    } else {
      labelEl.textContent = `${names[0]} +${names.length - 1} more`;
    }
  }
}

// Panel toggle — script loads at bottom of body so DOM is already ready; attach directly.
(function setupAudioSourceDropdown() {
  const btn = document.getElementById('audioSourceBtn');
  const panel = document.getElementById('audioSourcePanel');
  const selector = document.getElementById('audioSourceSelector');

  btn.addEventListener('click', (e) => {
    e.stopPropagation();
    const open = btn.getAttribute('aria-expanded') === 'true';
    btn.setAttribute('aria-expanded', String(!open));
    panel.hidden = open;
  });

  document.addEventListener('click', (e) => {
    if (!selector.contains(e.target)) {
      btn.setAttribute('aria-expanded', 'false');
      panel.hidden = true;
    }
  });

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      btn.setAttribute('aria-expanded', 'false');
      panel.hidden = true;
    }
  });
}());

async function populateProfileSelect() {
  try {
    const profiles = await invoke('list_profiles');
    const sel = document.getElementById('profileSelect');
    sel.innerHTML = '<option value="">No profile</option>' +
      profiles.map(p => `<option value="${p.id}">${esc(p.name)}</option>`).join('');
  } catch (e) { console.error('Failed to list profiles:', e); }
}

// --- Onboarding Wizard ---
let wizardCurrentStep = 1;
const WIZARD_TOTAL_STEPS = 4;

async function initWizard() {
  const config = await invoke('get_config');
  const hasKey = config.claude_api_key || config.openai_api_key || config.ai_provider === 'claude-cli';
  // Show wizard if setup not complete AND no usable AI provider configured
  if (!config.setup_complete && !hasKey) {
    await populateWizardAudio();
    showWizardStep(1);
    document.getElementById('wizardOverlay').hidden = false;
  }
}

async function populateWizardAudio() {
  const list = document.getElementById('wizardAudioList');
  list.innerHTML = '';
  // System Audio
  list.appendChild(buildWizardAudioItem('system', 'System Audio', true));
  try {
    const devices = await invoke('list_audio_devices');
    devices.forEach(([name]) => list.appendChild(buildWizardAudioItem(name, name, false)));
  } catch(e) { /* no devices listed, system audio only */ }
}

function buildWizardAudioItem(value, label, checked) {
  const el = document.createElement('label');
  el.className = 'wizard-audio-item';
  el.innerHTML = `<input type="checkbox" data-wizard-audio value="${value}" ${checked ? 'checked' : ''}><span>${esc(label)}</span>`;
  return el;
}

function showWizardStep(step) {
  wizardCurrentStep = step;
  for (let i = 1; i <= WIZARD_TOTAL_STEPS; i++) {
    const el = document.getElementById(`wizardStep${i}`);
    if (el) el.hidden = i !== step;
  }
  document.getElementById('wizardStepIndicator').textContent = `Step ${step} of ${WIZARD_TOTAL_STEPS}`;
  document.getElementById('wizardBackBtn').hidden = step === 1;
  document.getElementById('wizardNextBtn').textContent = step === WIZARD_TOTAL_STEPS ? 'Start using the app' : 'Next';

  // Show/hide AWS fields on step 2
  if (step === 2) {
    const selected = document.querySelector('input[name="wizardTranscription"]:checked')?.value || 'whisper';
    document.getElementById('wizardAwsFields').hidden = selected !== 'aws';
    document.querySelectorAll('input[name="wizardTranscription"]').forEach(r =>
      r.addEventListener('change', () => {
        document.getElementById('wizardAwsFields').hidden = r.value !== 'aws';
      })
    );
  }

  // Build summary on step 4
  if (step === 4) buildWizardSummary();
}

function buildWizardSummary() {
  const provider = document.querySelector('input[name="wizardProvider"]:checked')?.value || 'claude';
  const tx = document.querySelector('input[name="wizardTranscription"]:checked')?.value || 'aws';
  const audioSrcs = [...document.querySelectorAll('[data-wizard-audio]:checked')].map(c => c.closest('label').querySelector('span').textContent);
  const providerLabel = { claude: 'Claude (API key)', 'claude-cli': 'Claude CLI', openai: 'OpenAI' }[provider] || provider;
  const ul = document.getElementById('wizardSummary');
  ul.innerHTML = `
    <li>AI provider: <strong>${providerLabel}</strong></li>
    <li>Transcription: <strong>${tx === 'aws' ? 'AWS Transcribe' : 'Whisper (local)'}</strong></li>
    <li>Audio: <strong>${audioSrcs.join(', ') || 'None selected'}</strong></li>
  `;
}

function wizardProviderChanged() {
  const provider = document.querySelector('input[name="wizardProvider"]:checked')?.value || 'claude';
  const isCli = provider === 'claude-cli';
  document.getElementById('wizardApiKeySection').style.display = isCli ? 'none' : '';
  document.getElementById('wizardCliSection').style.display = isCli ? '' : 'none';
  document.getElementById('wizardConnectionStatus').textContent = '';
  document.getElementById('wizardConnectionStatus').className = 'wizard-status';
}

async function wizardNext() {
  if (wizardCurrentStep === WIZARD_TOTAL_STEPS) {
    await finishWizard();
    return;
  }

  // Step 1 validation: must have an API key entered (unless using CLI)
  if (wizardCurrentStep === 1) {
    const provider = document.querySelector('input[name="wizardProvider"]:checked').value;
    const config = await invoke('get_config');
    if (provider === 'claude-cli') {
      config.ai_provider = 'claude-cli';
    } else {
      const key = document.getElementById('wizardApiKey').value.trim();
      if (!key) {
        document.getElementById('wizardConnectionStatus').textContent = 'Enter an API key to continue.';
        document.getElementById('wizardConnectionStatus').className = 'wizard-status error';
        return;
      }
      if (provider === 'claude') config.claude_api_key = key;
      else config.openai_api_key = key;
      config.ai_provider = provider;
    }
    await invoke('save_config', { config });
  }

  // Step 2: save transcription config
  if (wizardCurrentStep === 2) {
    const tx = document.querySelector('input[name="wizardTranscription"]:checked').value;
    const config = await invoke('get_config');
    config.transcription_provider = tx;
    if (tx === 'aws') {
      config.aws_profile = document.getElementById('wizardAwsProfile').value.trim() || null;
      config.aws_region = document.getElementById('wizardAwsRegion').value.trim() || null;
    }
    await invoke('save_config', { config });
  }

  showWizardStep(wizardCurrentStep + 1);
}

function wizardBack() {
  if (wizardCurrentStep > 1) showWizardStep(wizardCurrentStep - 1);
}

async function wizardTestConnection() {
  const provider = document.querySelector('input[name="wizardProvider"]:checked').value;
  const statusEl = document.getElementById('wizardConnectionStatus');
  const btn = provider === 'claude-cli'
    ? document.getElementById('wizardTestCliBtn')
    : document.getElementById('wizardTestBtn');

  if (provider !== 'claude-cli') {
    const key = document.getElementById('wizardApiKey').value.trim();
    if (!key) return;
  }

  // Save provider + key so test_ai_connection can read from config
  const config = await invoke('get_config');
  let origKey = null;
  if (provider === 'claude-cli') {
    config.ai_provider = 'claude-cli';
  } else {
    const key = document.getElementById('wizardApiKey').value.trim();
    origKey = provider === 'claude' ? config.claude_api_key : config.openai_api_key;
    if (provider === 'claude') config.claude_api_key = key;
    else config.openai_api_key = key;
    config.ai_provider = provider;
  }
  await invoke('save_config', { config });

  btn.disabled = true;
  statusEl.textContent = 'Testing...';
  statusEl.className = 'wizard-status';

  try {
    const name = await invoke('test_ai_connection');
    statusEl.textContent = `✓ Connected to ${name}`;
    statusEl.className = 'wizard-status success';
  } catch (e) {
    statusEl.textContent = `✗ ${e}`;
    statusEl.className = 'wizard-status error';
    // Restore original key on failure (not applicable for CLI)
    if (provider !== 'claude-cli') {
      const cfg = await invoke('get_config');
      if (provider === 'claude') cfg.claude_api_key = origKey;
      else cfg.openai_api_key = origKey;
      await invoke('save_config', { config: cfg });
    }
  } finally {
    btn.disabled = false;
  }
}

async function finishWizard() {
  await invoke('mark_setup_complete');
  document.getElementById('wizardOverlay').hidden = true;
}

async function skipWizard() {
  await invoke('mark_setup_complete');
  document.getElementById('wizardOverlay').hidden = true;
}

// --- Init ---
(async () => {
  // Capture original text on focus for contenteditable elements
  document.addEventListener('focusin', (e) => {
    if (e.target.hasAttribute('contenteditable')) {
      e.target._originalText = e.target.innerText.trim();
    }
  });

  // Hide Windows-only settings on other platforms
  if (!navigator.userAgent.includes('Windows')) {
    document.querySelectorAll('.windows-only').forEach(el => el.style.display = 'none');
  }

  try {
    const state = await invoke('get_session_state');
    setMeetingControls(state.toLowerCase());
  } catch (e) {
    setMeetingControls('idle');
  }

  populateAudioSources();
  populateProfileSelect();
  await initWizard();
})();
