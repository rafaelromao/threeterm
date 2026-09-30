// Progressive enhancement: commands and navigation work without JavaScript.
for (const button of document.querySelectorAll('[data-copy]')) {
  const command = document.getElementById(button.dataset.copy);
  if (!command || !navigator.clipboard?.writeText) continue;
  button.hidden = false;
  button.addEventListener('click', async () => {
    const status = document.getElementById('copy-status');
    try {
      await navigator.clipboard.writeText(command.textContent.trim());
      status.textContent = 'Command copied. Paste it into your terminal when ready.';
      button.textContent = 'Copied';
      window.setTimeout(() => { button.textContent = 'Copy command'; }, 2500);
    } catch {
      status.textContent = 'Clipboard unavailable. Select and copy the command text above.';
    }
  });
}
