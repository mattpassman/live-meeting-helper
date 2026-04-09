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
  const micDevice = document.getElementById('micDevice').value || undefined;
  const audioSource = document.getElementById('captureSystem').checked ? 'both' : 'microphone';
  const profileName = document.getElementById('profileSelect').value || null;
  const title = document.getElementById('meetingTitle').value || undefined;
  try {
    const onNotes = new Channel();
    onNotes.onmessage = (notes) => {
      console.log('Notes received via channel');
      renderNotes(notes);
    };

    const onState = new Channel();
    onState.onmessage = (state) => {
      console.log('State received via channel:', state);
      const s = state.toLowerCase();
      setMeetingControls(s === 'completed' ? 'idle' : s);
    };

    await invoke('start_meeting', { audioSource, title, profileName, micDevice, onNotes, onState });
    setMeetingControls('active');
  } catch (e) {
    alert('Failed to start: ' + e);
  }
}

async function pauseMeeting() {
  try {
    await invoke('pause_meeting');
    setMeetingControls('paused');
  } catch (e) { alert(e); }
}

async function resumeMeeting() {
  try {
    await invoke('resume_meeting');
    setMeetingControls('active');
  } catch (e) { alert(e); }
}

async function stopMeeting() {
  try {
    setMeetingControls('idle');
    await invoke('stop_meeting');
  } catch (e) { alert(e); }
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
      html += `<div class="section ${updated ? 'updated' : ''}"><h2>Action Items</h2>${items}<button class="btn btn-add" onclick="addBlock('action_items')">+ Add Action Item</button></div>`;
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
      html += `<div class="section ${updated ? 'updated' : ''}"><h2>Decisions</h2>${decs}<button class="btn btn-add" onclick="addBlock('decisions')">+ Add Decision</button></div>`;
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
      html += `<div class="section ${updated ? 'updated' : ''}"><h2>Discussion Topics</h2>${topics}<button class="btn btn-add" onclick="addBlock('discussion_topics')">+ Add Topic</button></div>`;
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

function addBlock(section) {
  // Find the add button's parent section and insert an empty editable block before the button
  const btn = event.target;
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
  newEl.addEventListener('focusout', async (e) => {
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
      const all = [lastNotes.summary, ...(lastNotes.discussion_topics||[]).map(t=>t), ...(lastNotes.decisions||[]).map(d=>d), ...(lastNotes.action_items||[]).map(a=>a)];
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
      return `<div class="history-item" onclick="viewSession('${s.session_id}')">
        <div><span class="title">${esc(s.title)}</span><br><span class="meta">${date} · ${s.state}</span></div>
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
    if (session.transcript?.length) {
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
  } catch (e) { alert(e); }
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
  contentEl.innerHTML = '<em>Updating notes...</em>';
  try {
    const notes = await invoke('query_session', { sessionId: currentHistorySession, question });
    contentEl.innerHTML = '<em>Notes updated.</em>';
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
  } catch (e) { alert(e); }
}

async function saveProfileForm() {
  const id = document.getElementById('profileId').value || crypto.randomUUID();
  const name = document.getElementById('profileName').value.trim();
  if (!name) { alert('Name is required'); return; }
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
  } catch (e) { alert(e); }
}

async function deleteProfileById(id) {
  if (!confirm('Delete this profile?')) return;
  try {
    await invoke('delete_profile', { id });
    loadProfiles();
  } catch (e) { alert(e); }
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
    document.getElementById('settingsOpenAiKey').value = config.openai_api_key || '';
    document.getElementById('settingsOpenAiModel').value = config.openai_model || '';
    const txProvider = config.transcription_provider || 'aws';
    document.getElementById('settingsTranscriptionProvider').value = txProvider;
    const whisperPath = config.whisper_model_path || '';
    document.getElementById('settingsWhisperModelPath').value = whisperPath;
    document.getElementById('settingsAwsProfile').value = config.aws_profile || '';
    document.getElementById('settingsAwsRegion').value = config.aws_region || '';
    document.getElementById('settingsAudioDevice').value = config.audio_device || '';
    document.getElementById('settingsVerbose').checked = config.verbose_logging || false;
    toggleAiProviderFields();
    toggleTranscriptionFields();

    // Mark any already-downloaded model in the picker
    const modelIds = ['tiny.en', 'base.en', 'small.en', 'medium.en'];
    modelIds.forEach(id => {
      const statusEl = document.getElementById(`modelStatus-${id}`);
      const btn = document.querySelector(`[data-model-id="${id}"] button`);
      if (!statusEl || !btn) return;
      const key = id.replace('.', '-'); // e.g. "tiny-en" — just use includes on path
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

    const devices = await invoke('list_audio_devices');
    const dl = document.getElementById('audioDevicesList');
    if (devices.length) {
      dl.innerHTML = '<b>Available devices:</b><br>' + devices.map(([name, isDef]) =>
        `${name}${isDef ? ' (default)' : ''}`
      ).join('<br>');
    }
  } catch (e) { console.error(e); }
}

async function saveSettings() {
  const config = {
    ai_provider: document.getElementById('settingsAiProvider').value || 'claude',
    claude_api_key: document.getElementById('settingsClaudeApiKey').value || null,
    claude_model: document.getElementById('settingsClaudeModel').value || null,
    openai_api_key: document.getElementById('settingsOpenAiKey').value || null,
    openai_model: document.getElementById('settingsOpenAiModel').value || null,
    transcription_provider: document.getElementById('settingsTranscriptionProvider').value || 'aws',
    whisper_model_path: document.getElementById('settingsWhisperModelPath').value || null,
    aws_profile: document.getElementById('settingsAwsProfile').value || null,
    aws_region: document.getElementById('settingsAwsRegion').value || null,
    audio_device: document.getElementById('settingsAudioDevice').value || null,
    verbose_logging: document.getElementById('settingsVerbose').checked,
  };
  try {
    await invoke('save_config', { config });
    alert('Settings saved!');
  } catch (e) { alert(e); }
}

// --- Meeting screen dropdowns ---
async function populateMicDevices() {
  try {
    const devices = await invoke('list_audio_devices');
    const sel = document.getElementById('micDevice');
    sel.innerHTML = '<option value="">Default Microphone</option>' +
      devices.map(([name]) => `<option value="${name}">${esc(name)}</option>`).join('');
  } catch (e) { console.error('Failed to list audio devices:', e); }
}

async function populateProfileSelect() {
  try {
    const profiles = await invoke('list_profiles');
    const sel = document.getElementById('profileSelect');
    sel.innerHTML = '<option value="">No profile</option>' +
      profiles.map(p => `<option value="${p.id}">${esc(p.name)}</option>`).join('');
  } catch (e) { console.error('Failed to list profiles:', e); }
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

  populateMicDevices();
  populateProfileSelect();
})();
