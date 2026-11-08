const $ = id => document.getElementById(id);
let jobId = null, pages = [], polling = false;
const labels = {queued: 'En file', running: 'Extraction en cours', completed: 'Extraction terminée', failed: 'Extraction à vérifier', cancelled: 'Traitement annulé'};
function showError(message) { $('error').textContent = message; $('error').hidden = !message; }
async function request(url, options) {
  const response = await fetch(url, options);
  if (!response.ok) {
    let body; try { body = await response.json(); } catch { body = {}; }
    throw new Error(typeof body.detail === 'string' ? body.detail : `Requête refusée (${response.status}). Vérifiez les paramètres.`);
  }
  return response.json();
}
function tab(chunks) { $('chunks').hidden = !chunks; $('content').hidden = chunks; $('text-tab').classList.toggle('active', !chunks); $('chunks-tab').classList.toggle('active', chunks); }
function showPage() {
  const page = pages[Number($('page-select').value)];
  if (!page) return;
  $('content').textContent = page.content;
  $('chunks').replaceChildren();
  if (page.error) showError(`${page.error.code} : ${page.error.message}`);
  else if (page.warnings.length) showError(`À vérifier : ${page.warnings.join(', ')}`);
  else showError('');
  for (const chunk of page.chunks) {
    const card = document.createElement('div'); card.className = 'chunk'; card.tabIndex = 0; card.setAttribute('role', 'button');
    const meta = document.createElement('small');
    meta.textContent = `${chunk.kind} · caractères ${chunk.start_char}–${chunk.end_char}${chunk.token_count !== null ? ` · ${chunk.token_count} tokens` : ''}${chunk.oversized ? ' · bloc supérieur au budget' : ''}${chunk.section.length ? ' · ' + chunk.section.join(' / ') : ''}`;
    const text = document.createElement('p'); text.textContent = chunk.text;
    const id = document.createElement('code'); id.textContent = `id ${chunk.id.slice(0, 16)}…`;
    card.append(meta, text, id);
    function highlight() {
      tab(false); const mark = document.createElement('mark'); mark.textContent = chunk.text;
      // Python offsets count Unicode scalars; Array.from uses the same units.
      const chars = Array.from(page.content);
      $('content').replaceChildren(document.createTextNode(chars.slice(0, chunk.start_char).join('')), mark, document.createTextNode(chars.slice(chunk.end_char).join('')));
      mark.scrollIntoView({behavior: 'smooth', block: 'center'});
    }
    card.addEventListener('click', highlight);
    card.addEventListener('keydown', event => { if (event.key === 'Enter') highlight(); });
    $('chunks').append(card);
  }
}
async function poll(id) {
  polling = true;
  try {
    while (id === jobId) {
      const job = await request(`/jobs/${id}`);
      $('status').textContent = `${labels[job.status] || job.status}${job.pages ? ' · ' + job.pages + ' pages' : ''}`;
      if (['completed', 'failed', 'cancelled'].includes(job.status)) {
        if (job.error) showError(job.error);
        if (job.pages) {
          pages = await request(`/jobs/${id}/results?format=json`);
          $('page-count').textContent = pages.filter(p => p.page_index >= 0).length;
          $('chunk-count').textContent = pages.reduce((n,p) => n + p.chunks.length, 0);
          $('ocr-count').textContent = pages.filter(p => p.ocr_used).length;
          $('page-select').replaceChildren(...pages.map((p,i) => { const option = document.createElement('option'); option.value = i; option.textContent = `${p.source} · ${p.page_index < 0 ? 'Erreur du fichier' : 'Page ' + (p.page_index + 1)}`; return option; }));
          for (const format of ['json', 'jsonl', 'markdown']) $('download-' + format).href = `/jobs/${id}/results?format=${format}`;
          $('empty').hidden = true; $('results').hidden = false; showPage();
        }
        if (job.status === 'cancelled') showError('Traitement annulé. Les résultats affichés peuvent être partiels.');
        break;
      }
      await new Promise(resolve => setTimeout(resolve, 450));
    }
  } catch (error) { showError(error.message); }
  finally { polling = false; $('run').disabled = false; $('cancel').hidden = true; $('status').classList.remove('busy'); }
}
$('upload').addEventListener('submit', async event => {
  event.preventDefault(); if (polling) return;
  showError(''); $('run').disabled = true; $('results').hidden = true; $('empty').hidden = false;
  $('status').textContent = 'Envoi du document'; $('status').classList.add('busy');
  const data = new FormData(event.target); data.set('file', $('file').files[0]);
  try { const job = await request('/jobs', {method: 'POST', body: data}); jobId = job.id; $('cancel').hidden = false; $('cancel').disabled = false; await poll(jobId); }
  catch (error) { showError(error.message); $('run').disabled = false; $('status').textContent = 'Envoi refusé'; $('status').classList.remove('busy'); }
});
$('cancel').addEventListener('click', async () => { if (jobId) { try { await request(`/jobs/${jobId}`, {method: 'DELETE'}); $('cancel').disabled = true; } catch(error) { showError(error.message); } } });
$('page-select').addEventListener('change', showPage);
$('text-tab').addEventListener('click', () => tab(false));
$('chunks-tab').addEventListener('click', () => tab(true));
$('drop').addEventListener('dragover', event => { event.preventDefault(); $('drop').classList.add('drag'); });
$('drop').addEventListener('dragleave', () => $('drop').classList.remove('drag'));
$('drop').addEventListener('drop', event => { event.preventDefault(); $('drop').classList.remove('drag'); $('file').files = event.dataTransfer.files; });
