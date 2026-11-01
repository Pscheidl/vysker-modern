const mounted = new WeakMap()
const maximumDisplayMs = 2 * 60 * 1000

export function mount_rental_identity(root) {
  unmount_rental_identity(root)
  const open = root.querySelector('[data-identity-open]')
  const form = root.querySelector('[data-identity-form]')
  const password = root.querySelector('[data-identity-password]')
  const submit = root.querySelector('[data-identity-submit]')
  const result = root.querySelector('[data-identity-result]')
  const container = root.querySelector('[data-identity-image-container]')
  const enlarge = root.querySelector('[data-identity-enlarge]')
  const dialog = root.querySelector('[data-identity-dialog]')
  const enlargedContainer = root.querySelector('[data-identity-enlarged-container]')
  const minimize = root.querySelector('[data-identity-minimize]')
  const status = root.querySelector('[data-identity-status]')
  const timers = root.querySelectorAll('[data-identity-timer]')
  const error = root.querySelector('[data-identity-error]')
  const listeners = new AbortController()
  let request = null
  let objectUrl = null
  let image = null
  let deadline = 0
  let expiryTimeout = null
  let countdown = null

  function clear(notice = '') {
    request?.abort()
    request = null
    clearTimeout(expiryTimeout)
    clearInterval(countdown)
    expiryTimeout = null
    countdown = null
    deadline = 0
    password.value = ''
    error.textContent = ''
    image?.removeAttribute('src')
    image?.remove()
    image = null
    container.replaceChildren()
    enlargedContainer.replaceChildren()
    if (objectUrl) URL.revokeObjectURL(objectUrl)
    objectUrl = null
    if (dialog.open) dialog.close()
    form.hidden = true
    result.hidden = true
    open.hidden = false
    submit.disabled = false
    for (const timer of timers) timer.textContent = ''
    status.textContent = notice
  }

  function expire() {
    const restoreFocus = root.contains(document.activeElement)
    clear('Zobrazení dokladu vypršelo. Kopie byla ze stránky odstraněna. Pro další zobrazení zadejte znovu své heslo.')
    if (restoreFocus) open.focus()
  }

  function updateCountdown() {
    if (!deadline) return
    const remaining = deadline - Date.now()
    if (remaining <= 0) return expire()
    const seconds = Math.ceil(remaining / 1000)
    for (const timer of timers) {
      timer.textContent = `Automaticky se zavře za ${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`
    }
  }

  function minimizeImage() {
    if (!image || !objectUrl) return
    if (Date.now() >= deadline) return expire()
    if (dialog.open) dialog.close()
    container.replaceChildren(image)
    enlarge.focus({ preventScroll: true })
  }

  enlarge.addEventListener('click', () => {
    if (!image || !objectUrl || dialog.open) return
    if (Date.now() >= deadline) return expire()
    // Move the original image, keeping one URL and the original expiry deadline.
    enlargedContainer.replaceChildren(image)
    dialog.showModal()
    updateCountdown()
    minimize.focus({ preventScroll: true })
  }, { signal: listeners.signal })
  minimize.addEventListener('click', minimizeImage, { signal: listeners.signal })
  dialog.addEventListener('cancel', event => {
    event.preventDefault()
    minimizeImage()
  }, { signal: listeners.signal })
  dialog.addEventListener('click', event => {
    if (event.target !== dialog) return
    const bounds = dialog.getBoundingClientRect()
    if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) minimizeImage()
  }, { signal: listeners.signal })
  dialog.addEventListener('close', () => {
    // A queued close from an earlier reveal must not affect a newly opened one.
    if (!dialog.open && image?.parentElement === enlargedContainer) minimizeImage()
  }, { signal: listeners.signal })

  open.addEventListener('click', () => {
    clear()
    open.hidden = true
    form.hidden = false
    password.focus()
  }, { signal: listeners.signal })
  for (const close of root.querySelectorAll('[data-identity-close]')) {
    close.addEventListener('click', () => { clear(); open.focus() }, { signal: listeners.signal })
  }
  form.addEventListener('submit', async event => {
    event.preventDefault()
    if (request) return
    let secret = password.value
    password.value = ''
    error.textContent = ''
    submit.disabled = true
    const current = new AbortController()
    request = current
    try {
      const sessionResponse = await fetch('/api/v1/admin/session', { credentials: 'same-origin', cache: 'no-store', signal: current.signal })
      if (!sessionResponse.ok) throw new Error('Přihlášení vypršelo. Přihlaste se znovu.')
      const session = await sessionResponse.json()
      const responseRequest = fetch(root.dataset.identityUrl, {
        method: 'POST', credentials: 'same-origin', cache: 'no-store', signal: current.signal,
        headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': session.csrf_token },
        body: JSON.stringify({ password: secret }),
      })
      secret = ''
      const response = await responseRequest
      if (!response.ok) {
        const body = await response.json().catch(() => ({}))
        throw new Error(body.error || 'Doklad se nepodařilo zobrazit. Zkontrolujte heslo a zkuste to znovu.')
      }
      const blob = await response.blob()
      if (request !== current || current.signal.aborted || document.hidden) return
      objectUrl = URL.createObjectURL(blob)
      image = document.createElement('img')
      image.dataset.identityImage = ''
      image.className = 'rental-identity-image'
      image.alt = 'Kopie občanského průkazu'
      image.draggable = false
      const currentImage = image
      image.addEventListener('error', () => {
        if (image === currentImage) clear('Doklad se nepodařilo zobrazit. Zkuste to znovu.')
      }, { once: true, signal: listeners.signal })
      image.src = objectUrl
      container.replaceChildren(image)
      deadline = Date.now() + maximumDisplayMs
      expiryTimeout = setTimeout(expire, maximumDisplayMs)
      countdown = setInterval(updateCountdown, 1000)
      updateCountdown()
      form.hidden = true
      result.hidden = false
      status.textContent = 'Doklad je dočasně zobrazený. Nejpozději za dvě minuty se ze stránky odstraní.'
    } catch (failure) {
      if (!current.signal.aborted) error.textContent = failure.message || 'Doklad se nepodařilo zobrazit.'
    } finally {
      secret = ''
      if (request === current) {
        request = null
        submit.disabled = false
      }
    }
  }, { signal: listeners.signal })
  document.addEventListener('visibilitychange', () => {
    if (document.hidden && (objectUrl || request || !form.hidden)) {
      clear('Po opuštění stránky bylo zobrazení dokladu ukončeno. Pro další zobrazení zadejte znovu své heslo.')
    } else updateCountdown()
  }, { signal: listeners.signal })
  window.addEventListener('pagehide', () => clear(), { signal: listeners.signal })
  window.addEventListener('focus', updateCountdown, { signal: listeners.signal })
  mounted.set(root, () => { clear(); listeners.abort() })
}

export function unmount_rental_identity(root) {
  mounted.get(root)?.()
  mounted.delete(root)
}
