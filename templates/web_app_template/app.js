// Sovereign Notes - Application Logic
document.addEventListener('DOMContentLoaded', async () => {
  const noteTitle = document.getElementById('note-title');
  const noteBody = document.getElementById('note-body');
  const btnSave = document.getElementById('btn-save-note');
  const btnLoad = document.getElementById('btn-load-note');
  const batteryIndicator = document.getElementById('battery-indicator');
  const storageStatus = document.getElementById('storage-status');

  const NOTE_PATH = '/data/user/notes.txt';

  // 1. Fetch system battery via InterOS SDK
  try {
    const batt = await InterOS.telemetry.getBattery();
    if (batteryIndicator) {
      batteryIndicator.textContent = `Battery: ${batt.level}% (${batt.charging ? 'Charging' : 'Discharging'})`;
    }
  } catch (e) {
    console.warn('Telemetry query fallback:', e);
  }

  // 2. Load note from capability-governed VFS
  async function loadNote() {
    try {
      const content = await InterOS.vfs.readFile(NOTE_PATH);
      const lines = content.split('\n');
      noteTitle.value = lines[0] || 'Untitled Note';
      noteBody.value = lines.slice(1).join('\n');
      storageStatus.textContent = `Loaded from ${NOTE_PATH}`;
      InterOS.audio.beep(600, 0.03, 'sine');
    } catch (err) {
      storageStatus.textContent = 'Ready for new note';
    }
  }

  // 3. Save note to VFS
  async function saveNote() {
    const fullText = `${noteTitle.value.trim() || 'Untitled'}\n${noteBody.value}`;
    try {
      btnSave.textContent = 'Saving...';
      const bytes = await InterOS.vfs.writeFile(NOTE_PATH, fullText);
      btnSave.textContent = 'Saved!';
      storageStatus.textContent = `Saved ${bytes} bytes to ${NOTE_PATH}`;
      InterOS.audio.beep(800, 0.04, 'sine');
      InterOS.notifications.show('Note successfully saved to Sovereign VFS');
      setTimeout(() => { btnSave.textContent = 'Save to VFS'; }, 1500);
    } catch (err) {
      btnSave.textContent = 'Error';
      storageStatus.textContent = `Save failed: ${err.message}`;
      InterOS.audio.beep(300, 0.08, 'sawtooth');
      setTimeout(() => { btnSave.textContent = 'Save to VFS'; }, 1500);
    }
  }

  btnSave?.addEventListener('click', saveNote);
  btnLoad?.addEventListener('click', loadNote);

  // Initial load
  loadNote();
});
