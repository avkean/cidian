import {$, beginLoading, fetchJSON, escapeHTML} from "/shared.js";

const HANDWRITING_DELAY = 260;

function initializeHandwriting(onSelect) {
  const canvas = $('#handwriting-canvas');
  const context = canvas.getContext('2d');
  const status = $('#handwriting-status');
  const candidates = $('#handwriting-candidates');
  const idleText = 'Draw one character. Stroke order does not matter.';
  let strokes = [];
  let drawing = false;
  let timer = null;
  let request = null;
  let version = 0;
  let settle = null;

  function draw() {
    const ink = getComputedStyle(document.documentElement).getPropertyValue('--ink').trim() || '#1d1d1f';
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.strokeStyle = ink;
    context.fillStyle = ink;
    context.lineWidth = 9;
    context.lineCap = 'round';
    context.lineJoin = 'round';
    for (const stroke of strokes) {
      if (stroke.length === 1) {
        context.beginPath();
        context.arc(stroke[0][0] * 600, stroke[0][1] * 600, 4.5, 0, Math.PI * 2);
        context.fill();
        continue;
      }
      context.beginPath();
      context.moveTo(stroke[0][0] * 600, stroke[0][1] * 600);
      for (const [x, y] of stroke.slice(1)) context.lineTo(x * 600, y * 600);
      context.stroke();
    }
    if (!strokes.length && document.activeElement.matches('#undo-stroke, #clear-strokes')) canvas.focus({preventScroll: true});
    $('#undo-stroke').disabled = !strokes.length;
    $('#clear-strokes').disabled = !strokes.length;
    $('#stroke-count').textContent = `${strokes.length} ${strokes.length === 1 ? 'stroke' : 'strokes'}`;
  }

  function point(event) {
    const bounds = canvas.getBoundingClientRect();
    return [
      Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)),
      Math.max(0, Math.min(1, (event.clientY - bounds.top) / bounds.height)),
    ];
  }

  function cancelRecognition() {
    request?.abort();
    clearTimeout(timer);
    version++;
    settle?.();
  }

  // Disable stale choices before recognition starts.
  function holdChoices() {
    settle = beginLoading(candidates, 'Finding matching characters.');
  }

  function clear() {
    cancelRecognition();
    strokes = [];
    drawing = false;
    draw();
    candidates.innerHTML = '';
    showStatus(idleText);
  }

  function showStatus(text, quiet = false) {
    status.textContent = text;
    status.classList.toggle('visually-hidden', quiet);
  }

  function schedule() {
    cancelRecognition();
    if (!strokes.length) {
      candidates.innerHTML = '';
      showStatus(idleText);
      return;
    }
    holdChoices();
    timer = setTimeout(recognize, HANDWRITING_DELAY);
  }

  async function recognize() {
    const current = version;
    const controller = request = new AbortController();
    const finish = settle = beginLoading(candidates, 'Finding matching characters.', controller.signal);
    try {
      const data = await fetchJSON('/api/handwriting', {
        method: 'POST',
        headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({strokes}),
        signal: controller.signal,
      });
      if (current !== version) return;
      const found = (data.candidates || []).slice(0, 10);
      showStatus(found.length ? 'Choose a character.' : 'No match yet. Add a stroke, or clear and try again.', found.length > 0);
      finish(found.length ? 'Matching characters ready.' : 'No matching characters.');
      candidates.innerHTML = found.map(item => `<button type="button" class="candidate-button" data-candidate="${escapeHTML(item.character)}" aria-label="Insert ${escapeHTML(item.character)}" lang="zh">${escapeHTML(item.character)}</button>`).join('');
    } catch (error) {
      if (error.name !== 'AbortError' && current === version) {
        candidates.innerHTML = '';
        showStatus(`Handwriting is unavailable. ${error.message}`);
      }
    } finally {
      finish();
    }
  }

  // Prevent a lifted stroke from flinging the page and swallowing Chromium's next tap.
  canvas.addEventListener('touchstart', event => event.preventDefault(), {passive: false});
  canvas.addEventListener('pointerdown', event => {
    if (event.button !== 0 || drawing) return;
    event.preventDefault();
    canvas.setPointerCapture(event.pointerId);
    drawing = true;
    cancelRecognition();
    holdChoices();
    strokes.push([point(event)]);
    draw();
  });
  canvas.addEventListener('pointermove', event => {
    if (!drawing || !canvas.hasPointerCapture(event.pointerId)) return;
    for (const move of event.getCoalescedEvents?.() ?? [event]) {
      const next = point(move);
      const last = strokes.at(-1).at(-1);
      if (Math.hypot(next[0] - last[0], next[1] - last[1]) > 0.002) strokes.at(-1).push(next);
    }
    draw();
  });
  for (const type of ['pointerup', 'pointercancel']) {
    canvas.addEventListener(type, event => {
      if (!drawing || !canvas.hasPointerCapture(event.pointerId)) return;
      drawing = false;
      canvas.releasePointerCapture(event.pointerId);
      draw();
      schedule();
    });
  }
  $('#undo-stroke').addEventListener('click', () => {
    strokes.pop();
    draw();
    schedule();
  });
  $('#clear-strokes').addEventListener('click', clear);
  candidates.addEventListener('click', event => {
    const button = event.target.closest('[data-candidate]');
    if (!button) return;
    const character = button.dataset.candidate;
    if (onSelect(character) === false) return;
    clear();
    canvas.focus({preventScroll: true});
  });
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', draw);
  return {redraw: draw};
}

function canAddCharacter(input, character) {
  const limit = input.maxLength;
  const value = input.value.slice(0, input.selectionStart) + character + input.value.slice(input.selectionEnd);
  if (value.length <= limit) return true;
  $('#handwriting-status').textContent = `Search is limited to ${limit} characters.`;
  return false;
}

export {initializeHandwriting, canAddCharacter};
