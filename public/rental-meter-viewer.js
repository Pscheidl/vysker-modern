const mounted = new WeakMap()

export function mount_rental_meter_viewer(root) {
  unmount_rental_meter_viewer(root)
  const dialog = root.querySelector('[data-meter-dialog]')
  const title = root.querySelector('[data-meter-dialog-title]')
  const status = root.querySelector('[data-meter-dialog-status]')
  const container = root.querySelector('[data-meter-dialog-image]')
  const close = root.querySelector('[data-meter-dialog-close]')
  const listeners = new AbortController()
  let request = null
  let objectUrl = null
  let image = null
  let opener = null

  function clear(restoreFocus = false) {
    request?.abort()
    request = null
    image?.removeAttribute('src')
    image?.remove()
    image = null
    container.replaceChildren()
    if (objectUrl) URL.revokeObjectURL(objectUrl)
    objectUrl = null
    status.textContent = ''
    if (dialog.open) dialog.close()
    const previous = opener
    opener = null
    if (restoreFocus && previous?.isConnected) previous.focus({ preventScroll: true })
  }

  for (const button of root.querySelectorAll('[data-meter-photo-url]')) {
    button.addEventListener('click', async () => {
      clear()
      opener = button
      title.textContent = button.dataset.meterPhotoTitle
      status.textContent = 'Načítám fotografii…'
      dialog.showModal()
      close.focus({ preventScroll: true })
      const current = new AbortController()
      request = current
      try {
        const response = await fetch(button.dataset.meterPhotoUrl, {
          credentials: 'same-origin', cache: 'no-store', referrerPolicy: 'no-referrer', signal: current.signal,
        })
        if (!response.ok) throw new Error(response.status === 401 ? 'Přihlášení vypršelo. Přihlaste se znovu.' : 'Fotografii se nepodařilo načíst. Zavřete náhled a zkuste to znovu.')
        const blob = await response.blob()
        if (request !== current || current.signal.aborted || listeners.signal.aborted || !dialog.open) return
        objectUrl = URL.createObjectURL(blob)
        image = document.createElement('img')
        image.alt = button.dataset.meterPhotoTitle
        image.draggable = false
        const currentImage = image
        image.addEventListener('error', () => {
          if (image !== currentImage) return
          image.removeAttribute('src')
          image.remove()
          image = null
          if (objectUrl) URL.revokeObjectURL(objectUrl)
          objectUrl = null
          status.textContent = 'Fotografii se nepodařilo zobrazit. Zavřete náhled a zkuste to znovu.'
        }, { once: true, signal: listeners.signal })
        image.src = objectUrl
        container.replaceChildren(image)
        status.textContent = ''
      } catch (error) {
        if (!current.signal.aborted && request === current) status.textContent = error.message || 'Fotografii se nepodařilo načíst.'
      } finally {
        if (request === current) request = null
      }
    }, { signal: listeners.signal })
  }
  close.addEventListener('click', () => clear(true), { signal: listeners.signal })
  dialog.addEventListener('cancel', event => {
    event.preventDefault()
    clear(true)
  }, { signal: listeners.signal })
  dialog.addEventListener('click', event => {
    if (event.target !== dialog) return
    const bounds = dialog.getBoundingClientRect()
    if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) clear(true)
  }, { signal: listeners.signal })
  dialog.addEventListener('close', () => {
    if (!dialog.open) clear(true)
  }, { signal: listeners.signal })
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) clear()
  }, { signal: listeners.signal })
  window.addEventListener('pagehide', () => clear(), { signal: listeners.signal })
  mounted.set(root, () => { clear(); listeners.abort() })
}

export function unmount_rental_meter_viewer(root) {
  mounted.get(root)?.()
  mounted.delete(root)
}
