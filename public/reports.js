// Local Leaflet integration. Geolocation is requested only by the mobile button.
let leafletPromise
let clusterPromise
const mounted = new WeakMap()
// A draft survives SPA history navigation only in this page's memory.
let reportDraft = null
const MAX_PHOTO_BYTES = 15_000_000
const MAX_TOTAL_PHOTO_BYTES = 50_000_000

function leaflet() {
  if (window.L) return Promise.resolve(window.L)
  if (!leafletPromise) {
    leafletPromise = new Promise((resolve, reject) => {
      const script = document.createElement('script')
      script.src = '/vendor/leaflet-1.9.4/leaflet.js'
      script.onload = () => resolve(window.L)
      script.onerror = () => {
        leafletPromise = undefined
        script.remove()
        reject(new Error('Mapu se nepodařilo načíst. Místo můžete zadat adresou.'))
      }
      document.head.append(script)
    })
  }
  return leafletPromise
}

function markerClusters(L) {
  if (L.markerClusterGroup) return Promise.resolve(L)
  if (!clusterPromise) {
    clusterPromise = new Promise((resolve, reject) => {
      const script = document.createElement('script')
      const fail = () => {
        clearTimeout(timeout)
        clusterPromise = undefined
        script.remove()
        reject(new Error('Mapu hlášení se nepodařilo načíst. Hlášení najdete v seznamu pod mapou.'))
      }
      const timeout = setTimeout(fail, 10000)
      script.src = '/vendor/leaflet-markercluster-1.5.3/leaflet.markercluster.js'
      script.onload = () => {
        clearTimeout(timeout)
        if (L.markerClusterGroup) resolve(L)
        else fail()
      }
      script.onerror = fail
      document.head.append(script)
    })
  }
  return clusterPromise
}

function newMap(L, element, center, zoom) {
  const map = L.map(element, { scrollWheelZoom: false, zoomControl: false }).setView(center, zoom)
  L.tileLayer('https://tile.openstreetmap.org/{z}/{x}/{y}.png', {
    maxZoom: 19,
    attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap contributors</a>',
    // OSM requires the embedding site's referrer, without disclosing its path.
    referrerPolicy: 'strict-origin-when-cross-origin',
  }).addTo(map)
  L.control.zoom({
    position: 'topright',
    zoomInTitle: 'Přiblížit mapu',
    zoomOutTitle: 'Oddálit mapu',
  }).addTo(map)
  return map
}

// Coordinate pairs are handled locally and are never sent to address search.
function coordinateInput(value) {
  const input = value.trim()
  if (!input || !/^[\s+\-\d.,;]+$/.test(input) || !/[\d]/.test(input)) return null
  const number = '[+-]?(?:\\d+(?:\\.\\d*)?|\\.\\d+)'
  let parts = null
  if (input.includes(';')) {
    const pair = input.split(';').map(part => part.trim().replace(',', '.'))
    if (pair.length === 2 && pair.every(part => new RegExp(`^${number}$`).test(part))) parts = pair
  } else {
    const pair = input.match(new RegExp(`^(${number})(?:\\s*,\\s*|\\s+)(${number})$`))
    if (pair) parts = pair.slice(1)
  }
  if (!parts) {
    // A single number can still be an address number or an unfinished coordinate.
    if (!/[,;]|\d\s+[+\-\d.]/.test(input)) return null
    return { error: 'Souřadnice zadejte jako zeměpisnou šířku a délku, například 50.529509, 15.158555. Pro desetinné čárky použijte mezi čísly středník.' }
  }
  const [latitude, longitude] = parts.map(Number)
  if (!Number.isFinite(latitude) || !Number.isFinite(longitude) || Math.abs(latitude) > 90 || Math.abs(longitude) > 180) {
    return { error: 'Zeměpisná šířka musí být od −90 do 90 a délka od −180 do 180.' }
  }
  return { latitude, longitude }
}

function mobilePhone() {
  return navigator.userAgentData?.mobile === true || /Android.*Mobile|iPhone|iPod|Windows Phone/i.test(navigator.userAgent)
}

export function unmount_report(root) {
  const state = mounted.get(root)
  if (!state) return
  state.saveDraft?.()
  state.active = false
  state.listeners?.abort()
  state.map?.remove()
  clearTimeout(state.searchTimer)
  clearTimeout(state.searchTimeout)
  clearTimeout(state.leaveTimer)
  state.searchRequest?.abort()
  for (const photo of state.photos || []) URL.revokeObjectURL(photo.url)
  mounted.delete(root)
}

export function mount_report_form(root) {
  if (mounted.has(root)) return
  const state = { active: true, listeners: new AbortController(), map: null, marker: null, photos: [], sending: false, submitted: false, discardDraft: false, allowUnload: false, suggestions: [], selectedSuggestion: -1, searchSequence: 0, submissionToken: null, tokenRequest: null, tokenSubmitted: false }
  mounted.set(root, state)
  const $ = id => root.querySelector(`#${id}`)
  const listen = (element, type, fn) => element.addEventListener(type, fn, { signal: state.listeners.signal })
  const form = $('report-form')
  const fields = $('report-fields')
  const address = $('report-address')
  const latitude = $('report-latitude')
  const longitude = $('report-longitude')
  const geo = $('report-geolocate')
  const clear = $('report-map-clear')
  const center = $('report-map-center')
  const locationStatus = $('report-location-status')
  const error = $('report-submit-error')
  const submit = $('report-submit')
  const photo = $('report-photo')
  const addressResults = $('report-address-results')
  const addressStatus = $('report-address-status')

  function hasDraft() {
    return !state.submitted && (state.photos.length > 0 || latitude.value || longitude.value
      || $('report-description').value.trim() || address.value.trim() || $('report-reporter').value.trim()
      || $('report-email').value.trim() || $('report-phone').value.trim())
  }

  state.saveDraft = () => {
    if (state.submitted || state.discardDraft) {
      reportDraft = null
    } else if (hasDraft()) {
      reportDraft = {
        description: $('report-description').value,
        address: address.value,
        reporter: $('report-reporter').value,
        email: $('report-email').value,
        phone: $('report-phone').value,
        latitude: latitude.value,
        longitude: longitude.value,
        photos: state.photos.map(item => item.file),
        submissionToken: state.submissionToken,
        tokenSubmitted: state.tokenSubmitted,
      }
    }
  }
  listen(form, 'input', () => { state.discardDraft = false })
  listen(form, 'change', () => { state.discardDraft = false })

  listen(window, 'beforeunload', event => {
    if (!state.active || !hasDraft() || state.allowUnload) return
    event.preventDefault()
    event.returnValue = ''
  })
  document.addEventListener('click', event => {
    if (!state.active || !hasDraft() || event.defaultPrevented || event.button !== 0
      || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
    const link = event.target instanceof Element ? event.target.closest('a[href]') : null
    if (!link || link.hasAttribute('download')) return
    const target = link.getAttribute('target')?.toLowerCase()
    if (target && !['_self', '_parent', '_top'].includes(target)) return
    let destination
    try { destination = new URL(link.href, window.location.href) } catch (_) { return }
    if (!['http:', 'https:'].includes(destination.protocol) || destination.href === window.location.href) return
    // Map zoom controls use href="#". Same-page fragments never leave the form.
    if (destination.origin === window.location.origin && destination.pathname === window.location.pathname
      && destination.search === window.location.search) return
    if (!window.confirm('Máte rozpracované hlášení. Opravdu chcete odejít? Vyplněné údaje a fotografie se neuloží.')) {
      event.preventDefault()
      event.stopImmediatePropagation()
      return
    }
    // The click already received confirmation, so avoid a second native prompt.
    state.allowUnload = true
    state.discardDraft = true
    reportDraft = null
    clearTimeout(state.leaveTimer)
    state.leaveTimer = setTimeout(() => { state.allowUnload = false }, 1500)
  }, { capture: true, signal: state.listeners.signal })

  function closeSuggestions() {
    addressResults.hidden = true
    address.setAttribute('aria-expanded', 'false')
    address.removeAttribute('aria-activedescendant')
    state.selectedSuggestion = -1
  }

  function cancelSearch() {
    clearTimeout(state.searchTimer)
    clearTimeout(state.searchTimeout)
    state.searchRequest?.abort()
    state.searchSequence += 1
  }

  function selectSuggestion(index) {
    const suggestion = state.suggestions[index]
    if (!suggestion || state.sending) return
    cancelSearch()
    address.value = suggestion.label
    address.setCustomValidity('')
    selectPoint(suggestion.latitude, suggestion.longitude, false)
    state.map?.setView([suggestion.latitude, suggestion.longitude], 17)
    closeSuggestions()
    addressStatus.textContent = 'Adresa vybrána. Při posunu špendlíku se do pole zapíší přesné souřadnice.'
  }

  function highlightSuggestion(index) {
    state.selectedSuggestion = index
    for (const [position, option] of [...addressResults.children].entries()) {
      option.setAttribute('aria-selected', String(position === index))
    }
    const option = addressResults.children[index]
    if (option) {
      address.setAttribute('aria-activedescendant', option.id)
      option.scrollIntoView({ block: 'nearest' })
    }
  }

  function showSuggestions(suggestions) {
    state.suggestions = suggestions
    state.selectedSuggestion = -1
    addressResults.replaceChildren()
    for (const [index, suggestion] of suggestions.entries()) {
      const option = document.createElement('li')
      option.id = `report-address-option-${index}`
      option.setAttribute('role', 'option')
      option.setAttribute('aria-selected', 'false')
      option.textContent = suggestion.label
      // Keep keyboard focus on the combobox while choosing with a pointer.
      option.addEventListener('pointerdown', event => event.preventDefault())
      option.addEventListener('click', () => selectSuggestion(index))
      addressResults.append(option)
    }
    addressResults.hidden = suggestions.length === 0
    address.setAttribute('aria-expanded', String(suggestions.length > 0))
    address.removeAttribute('aria-activedescendant')
    addressStatus.textContent = suggestions.length
      ? `Počet nalezených adres: ${suggestions.length}. Vyberte adresu nebo pokračujte v psaní.`
      : 'Adresu jsme nenašli. Upřesněte ji, ponechte vlastní popis místa nebo použijte mapu.'
  }

  async function searchAddresses(query, sequence) {
    if (!state.active || state.sending || sequence !== state.searchSequence) return
    const request = new AbortController()
    state.searchRequest = request
    state.searchTimeout = setTimeout(() => request.abort(), 6000)
    addressStatus.textContent = 'Vyhledávám adresy…'
    try {
      const response = await fetch(`/api/v1/report-addresses?q=${encodeURIComponent(query)}`, {
        credentials: 'same-origin', signal: request.signal,
      })
      if (!response.ok) throw new Error('Address search unavailable')
      const result = await response.json()
      if (!state.active || sequence !== state.searchSequence || document.activeElement !== address) return
      if (!Array.isArray(result)) throw new Error('Invalid address response')
      const suggestions = result.filter(item => typeof item?.label === 'string' && item.label.length <= 300
        && Number.isFinite(item.latitude) && Math.abs(item.latitude) <= 90
        && Number.isFinite(item.longitude) && Math.abs(item.longitude) <= 180).slice(0, 8)
      showSuggestions(suggestions)
    } catch (_) {
      if (state.active && sequence === state.searchSequence) {
        closeSuggestions()
        addressStatus.textContent = 'Našeptávání teď není dostupné. Adresu můžete vyplnit ručně nebo označit místo na mapě.'
      }
    } finally {
      if (sequence === state.searchSequence) {
        clearTimeout(state.searchTimeout)
        state.searchRequest = null
      }
    }
  }

  listen(address, 'input', () => {
    cancelSearch()
    closeSuggestions()
    addressStatus.textContent = ''
    address.setCustomValidity('')
    const query = address.value.trim()
    const coordinates = coordinateInput(query)
    if (coordinates) {
      if (coordinates.error) {
        address.setCustomValidity(coordinates.error)
        addressStatus.textContent = coordinates.error
      } else {
        selectPoint(coordinates.latitude, coordinates.longitude, false)
        state.map?.setView([coordinates.latitude, coordinates.longitude], 17)
        addressStatus.textContent = 'Souřadnice byly použity pro umístění špendlíku.'
      }
      return
    }
    // An unfinished numeric pair must not leak its first coordinate to Photon.
    if (query.length < 3 || /^[\s+\-\d.,;]+$/.test(query)) return
    if ([...query].length > 200) {
      addressStatus.textContent = 'Delší popis místa můžete ponechat. Našeptávání vyhledává pouze kratší adresy.'
      return
    }
    const sequence = state.searchSequence
    state.searchTimer = setTimeout(() => searchAddresses(query, sequence), 500)
  })
  listen(address, 'keydown', event => {
    if (event.key === 'Escape') {
      cancelSearch()
      closeSuggestions()
      addressStatus.textContent = ''
      return
    }
    if (addressResults.hidden || state.sending) return
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      const count = state.suggestions.length
      const next = state.selectedSuggestion < 0
        ? (event.key === 'ArrowDown' ? 0 : count - 1)
        : (state.selectedSuggestion + (event.key === 'ArrowDown' ? 1 : -1) + count) % count
      highlightSuggestion(next)
    } else if (event.key === 'Enter') {
      event.preventDefault()
      selectSuggestion(Math.max(0, state.selectedSuggestion))
    } else if (event.key === 'Tab') closeSuggestions()
  })
  listen(address, 'blur', () => {
    cancelSearch()
    closeSuggestions()
    const coordinates = coordinateInput(address.value)
    if (coordinates && !coordinates.error) selectPoint(coordinates.latitude, coordinates.longitude)
    else if (addressStatus.textContent === 'Vyhledávám adresy…') addressStatus.textContent = ''
  })

  function showError(message) {
    error.textContent = message
    error.hidden = false
    error.focus()
  }

  function markerAt(lat, lng) {
    if (!state.map || !window.L) return
    if (!state.marker) {
      state.marker = window.L.marker([lat, lng], { draggable: !state.sending, title: 'Místo závady', alt: 'Špendlík místa závady' }).addTo(state.map)
      state.marker.on('dragend', () => {
        const point = state.marker.getLatLng().wrap()
        selectPoint(point.lat, point.lng)
      })
    } else state.marker.setLatLng([lat, lng])
  }

  function selectPoint(lat, lng, updateAddress = true) {
    if (!state.active || state.sending || !Number.isFinite(lat) || !Number.isFinite(lng)
      || Math.abs(lat) > 90 || Math.abs(lng) > 180) return
    state.discardDraft = false
    latitude.value = lat.toFixed(6)
    longitude.value = lng.toFixed(6)
    if (updateAddress) {
      cancelSearch()
      closeSuggestions()
      address.value = `${latitude.value}, ${longitude.value}`
      address.setCustomValidity('')
      addressStatus.textContent = 'Pole obsahuje přesné souřadnice špendlíku.'
    }
    clear.hidden = false
    locationStatus.textContent = `Místo označeno: ${latitude.value}, ${longitude.value}. Špendlík můžete posunout.`
    markerAt(lat, lng)
  }

  listen(clear, 'click', () => {
    cancelSearch()
    closeSuggestions()
    if (coordinateInput(address.value)) {
      address.value = ''
      address.setCustomValidity('')
      addressStatus.textContent = ''
    }
    if (state.marker) state.marker.remove()
    state.marker = null
    latitude.value = ''
    longitude.value = ''
    clear.hidden = true
    locationStatus.textContent = 'Špendlík byl odebrán. Vyplňte adresu nebo znovu označte místo.'
  })
  listen(center, 'click', () => {
    if (!state.map) return
    const point = state.map.getCenter().wrap()
    selectPoint(point.lat, point.lng)
  })

  geo.hidden = !mobilePhone()
  listen(geo, 'click', () => {
    if (!mobilePhone() || geo.disabled) return
    if (!window.isSecureContext || !navigator.geolocation) {
      locationStatus.textContent = 'Zjištění polohy není dostupné. Otevřete web přes HTTPS, nebo vyplňte adresu či označte místo na mapě.'
      return
    }
    geo.disabled = true
    locationStatus.textContent = 'Zjišťuji polohu. Povolte přístup k poloze v prohlížeči.'
    navigator.geolocation.getCurrentPosition(position => {
      if (!state.active) return
      geo.disabled = false
      if (state.sending) return
      const { latitude: lat, longitude: lng, accuracy } = position.coords
      selectPoint(lat, lng)
      state.map?.setView([lat, lng], 17)
      locationStatus.textContent = `Poloha získána s přesností přibližně ${Math.round(accuracy)} m. Zkontrolujte špendlík a případně jej posuňte.`
    }, failure => {
      if (!state.active) return
      geo.disabled = false
      if (state.sending) return
      const message = failure.code === 1 ? 'Přístup k poloze nebyl povolen.' : failure.code === 3 ? 'Zjištění polohy trvalo příliš dlouho.' : 'Polohu se nepodařilo zjistit.'
      locationStatus.textContent = `${message} Zadejte adresu nebo označte místo na mapě.`
    }, { enableHighAccuracy: true, timeout: 15000, maximumAge: 0 })
  })

  function renderPhotos() {
    const preview = $('report-photo-preview')
    preview.replaceChildren()
    preview.hidden = state.photos.length === 0
    for (const item of state.photos) {
      const figure = document.createElement('figure')
      figure.className = 'report-photo-preview-item'
      figure.dataset.reportPhoto = item.file.name
      const img = document.createElement('img')
      img.src = item.url
      img.alt = `Náhled fotografie ${item.file.name}`
      const caption = document.createElement('figcaption')
      caption.textContent = item.file.name
      const remove = document.createElement('button')
      remove.type = 'button'
      remove.className = 'text-link'
      remove.textContent = 'Odebrat fotografii'
      remove.setAttribute('aria-label', `Odebrat fotografii ${item.file.name}`)
      remove.addEventListener('click', () => {
        if (state.sending) return
        URL.revokeObjectURL(item.url)
        state.photos = state.photos.filter(selected => selected !== item)
        renderPhotos()
        photo.focus()
      })
      figure.append(img, caption, remove)
      preview.append(figure)
    }
    const size = state.photos.reduce((sum, item) => sum + item.file.size, 0)
    $('report-photo-status').textContent = state.photos.length
      ? `Vybrané fotografie: ${state.photos.length}. Celkem ${(size / 1_000_000).toLocaleString('cs-CZ', { maximumFractionDigits: 2 })} MB z 50 MB.`
      : ''
  }

  listen(photo, 'change', () => {
    const selected = [...(photo.files || [])]
    // Files live in our collection so adding more or retrying does not discard them.
    photo.value = ''
    const photoError = $('report-photo-error')
    photoError.hidden = true
    if (!selected.length) return
    let message = ''
    for (const file of selected) {
      if (file.size > MAX_PHOTO_BYTES) {
        message = `Fotografie „${file.name}“ je příliš velká. Jedna fotografie může mít nejvýše 15 MB.`
        break
      }
      if (!/^image\/(jpeg|png|webp)$/.test(file.type) && (file.type || !/\.(jpe?g|png|webp)$/i.test(file.name))) {
        message = `Soubor „${file.name}“ není podporovaný. Vyberte fotografii JPG, PNG nebo WebP.`
        break
      }
    }
    const total = state.photos.reduce((sum, item) => sum + item.file.size, 0) + selected.reduce((sum, file) => sum + file.size, 0)
    if (!message && total > MAX_TOTAL_PHOTO_BYTES) message = 'Fotografie dohromady přesahují 50 MB. Vyberte méně fotografií nebo nejprve některou odeberte.'
    if (message) {
      photoError.textContent = message
      photoError.hidden = false
      return
    }
    state.photos.push(...selected.map(file => ({ file, url: URL.createObjectURL(file) })))
    renderPhotos()
  })

  async function prepareSubmissionToken(force = false) {
    // A response may have been lost after saving. Reuse that token even if its
    // original lifetime ended so a retry can retrieve the existing report.
    if (!force && state.submissionToken && (state.tokenSubmitted
      || state.submissionToken.expires_at * 1000 > Date.now() + 5000)) return state.submissionToken.token
    if (state.tokenRequest) return state.tokenRequest
    state.submissionToken = null
    state.tokenSubmitted = false
    state.tokenRequest = (async () => {
      const controller = new AbortController()
      const timeout = setTimeout(() => controller.abort(), 15000)
      try {
        const response = await fetch('/api/v1/report-submission-token', {
          method: 'POST', credentials: 'same-origin', signal: controller.signal,
        })
        const result = await response.json().catch(() => null)
        if (!response.ok) throw new Error(result?.error || 'Odeslání nyní nelze připravit. Zkuste to prosím znovu. Vyplněné údaje a fotografie zůstávají ve formuláři.')
        if (typeof result?.token !== 'string' || !result.token || !Number.isFinite(result.expires_at)) {
          throw new Error('Odeslání nyní nelze připravit. Zkuste to prosím znovu. Vyplněné údaje a fotografie zůstávají ve formuláři.')
        }
        state.submissionToken = result
        return result.token
      } catch (failure) {
        if (failure instanceof TypeError || failure.name === 'AbortError') {
          throw new Error('Nepodařilo se spojit se serverem. Zkuste odeslání znovu. Vyplněné údaje a fotografie zůstávají ve formuláři.')
        }
        throw failure
      } finally {
        clearTimeout(timeout)
        state.tokenRequest = null
      }
    })()
    return state.tokenRequest
  }

  listen(form, 'submit', async event => {
    event.preventDefault()
    if (state.sending || !form.reportValidity()) return
    error.hidden = true
    if (!address.value.trim() && (!latitude.value || !longitude.value)) {
      showError('Vyplňte adresu nebo označte místo na mapě.')
      address.focus()
      return
    }
    cancelSearch()
    closeSuggestions()
    const body = new FormData()
    // Place the proof before photographs so the server can reject an invalid
    // request before it reads or decodes a large upload.
    body.append('submission_token', 'pending')
    for (const [key, value] of new FormData(form)) {
      if (key !== 'photo') body.append(key, value)
    }
    for (const item of state.photos) body.append('photo', item.file)
    for (const [key, value] of [...body.entries()]) {
      if (typeof value === 'string' && !value.trim()) body.delete(key)
      if (value instanceof File && !value.size && !value.name) body.delete(key)
    }
    // This comes from the browser's standard locale setting, without any
    // device probing or access to an operating-system/profile user name.
    try { body.set('client_timezone', Intl.DateTimeFormat().resolvedOptions().timeZone || '') } catch (_) {}
    state.sending = true
    fields.disabled = true
    state.marker?.dragging?.disable()
    submit.textContent = 'Odesílám…'
    try {
      body.set('submission_token', await prepareSubmissionToken())
      let response
      let result
      for (let attempt = 0; attempt < 2; attempt += 1) {
        state.tokenSubmitted = true
        response = await fetch('/api/v1/reports', { method: 'POST', body, credentials: 'same-origin', headers: { 'X-Report-Token': body.get('submission_token') } })
        result = await response.json().catch(() => null)
        if (attempt === 0 && response.status === 403 && result?.code === 'report_token_invalid') {
          // Only this explicit rejection proves that this attempt was not saved.
          body.set('submission_token', await prepareSubmissionToken(true))
          continue
        }
        break
      }
      if (!response.ok) throw new Error(result?.error || (response.status === 413 ? 'Upload je příliš velký. Jedna fotografie může mít nejvýše 15 MB a všechny dohromady 50 MB.' : 'Hlášení se nepodařilo odeslat. Zkuste to znovu.'))
      if (!Number.isSafeInteger(result?.id) || result.id < 1) throw new Error('Server vrátil neočekávanou odpověď. Před opakováním zkontrolujte seznam hlášení.')
      state.submitted = true
      reportDraft = null
      if (state.active) window.location.assign(`/hlaseni-zavad/${result.id}?odeslano=1`)
    } catch (failure) {
      if (state.active) {
        fields.disabled = false
        state.marker?.dragging?.enable()
        submit.textContent = 'Odeslat hlášení'
        state.sending = false
        showError(failure instanceof TypeError ? 'Spojení se serverem se přerušilo. Zkuste odeslání znovu. Vyplněné údaje a fotografie zůstávají ve formuláři a při opakování stejného hlášení během 24 hodin zabráníme duplicitě.' : failure.message)
      }
    }
  })
  if (reportDraft) {
    const draft = reportDraft
    reportDraft = null
    $('report-description').value = draft.description
    address.value = draft.address
    $('report-reporter').value = draft.reporter
    $('report-email').value = draft.email || ''
    $('report-phone').value = draft.phone || ''
    state.submissionToken = draft.submissionToken || null
    state.tokenSubmitted = draft.tokenSubmitted || false
    state.photos = draft.photos.map(file => ({ file, url: URL.createObjectURL(file) }))
    renderPhotos()
    if (draft.latitude && draft.longitude) selectPoint(Number(draft.latitude), Number(draft.longitude), false)
    const coordinates = coordinateInput(address.value)
    if (coordinates?.error) {
      address.setCustomValidity(coordinates.error)
      addressStatus.textContent = coordinates.error
    }
    $('report-draft-status').hidden = false
  }
  fields.disabled = false
  // Preparing the token must never delay typing or choosing photographs.
  prepareSubmissionToken().catch(() => {})

  leaflet().then(L => {
    if (!state.active) return
    state.map = newMap(L, $('report-map'), [50.5295088, 15.158555], 17)
    state.map.on('click', event => {
      if (state.sending) return
      const point = event.latlng.wrap()
      selectPoint(point.lat, point.lng)
    })
    center.disabled = false
    if (latitude.value && longitude.value) {
      markerAt(Number(latitude.value), Number(longitude.value))
      state.map.setView([Number(latitude.value), Number(longitude.value)], 17)
    }
  }).catch(failure => {
    if (state.active) {
      $('report-map').textContent = failure.message
      locationStatus.textContent = 'Mapu nelze načíst. Hlášení můžete odeslat s vyplněnou adresou.'
    }
  })
}

export function mount_report_map(root) {
  if (mounted.has(root)) return
  const state = { active: true, map: null }
  mounted.set(root, state)
  leaflet().then(L => {
    if (!state.active) return
    const center = [Number(root.dataset.latitude), Number(root.dataset.longitude)]
    state.map = newMap(L, root, center, 17)
    L.marker(center, { title: 'Místo závady', alt: 'Špendlík místa závady' }).addTo(state.map)
  }).catch(() => {
    if (state.active) root.textContent = 'Mapu se nepodařilo načíst. Použijte odkaz pod mapou.'
  })
}


function overviewPopup(point) {
  const article = document.createElement('article')
  article.className = 'report-map-popup'
  article.dataset.reportId = String(point.id)
  const heading = document.createElement('h3')
  const title = document.createElement('a')
  title.href = `/hlaseni-zavad/${point.id}`
  title.textContent = point.description
  heading.append(title)
  const meta = document.createElement('div')
  meta.className = 'report-card-meta'
  const date = document.createElement('time')
  date.dateTime = point.created_at
  const parsedDate = new Date(point.created_at)
  date.textContent = Number.isNaN(parsedDate.getTime()) ? point.created_at : new Intl.DateTimeFormat('cs-CZ', {
    dateStyle: 'medium', timeStyle: 'short', timeZone: 'Europe/Prague',
  }).format(parsedDate)
  const status = document.createElement('span')
  const knownStatus = ['reported', 'in_progress', 'resolved'].includes(point.status) ? point.status : 'reported'
  status.className = `report-status report-status-${knownStatus}`
  status.textContent = { reported: 'Nahlášeno', in_progress: 'V řešení', resolved: 'Vyřešeno' }[knownStatus]
  meta.append(date, status)
  article.append(heading, meta)
  if (point.address) {
    const address = document.createElement('p')
    address.className = 'report-map-popup-address'
    address.textContent = point.address
    article.append(address)
  }
  const link = document.createElement('a')
  link.className = 'report-map-detail-link'
  link.href = `/hlaseni-zavad/${point.id}`
  link.textContent = 'Zobrazit detail'
  article.append(link)
  if (point.has_photo) {
    const photo = document.createElement('img')
    photo.src = `/api/v1/reports/${point.id}/photo`
    photo.alt = 'Fotografie nahlášené závady'
    photo.loading = 'lazy'
    photo.width = 220
    photo.height = 120
    article.append(photo)
  }
  return article
}

export function mount_report_overview(root, serializedPoints) {
  const previous = mounted.get(root)
  if (previous?.serializedPoints === serializedPoints) return
  if (previous) unmount_report(root)
  const state = { active: true, map: null, serializedPoints }
  mounted.set(root, state)
  let points
  try {
    points = JSON.parse(serializedPoints).filter(point => Number.isSafeInteger(point.id) && point.id > 0
      && typeof point.description === 'string' && typeof point.created_at === 'string'
      && Number.isFinite(point.latitude) && Math.abs(point.latitude) <= 90
      && Number.isFinite(point.longitude) && Math.abs(point.longitude) <= 180).slice(0, 2000)
  } catch (_) {
    root.textContent = 'Mapu hlášení se nepodařilo načíst. Hlášení najdete v seznamu pod mapou.'
    return
  }
  leaflet().then(markerClusters).then(L => {
    if (!state.active) return
    state.map = newMap(L, root, [50.5295088, 15.158555], 14)
    const group = L.markerClusterGroup({
      showCoverageOnHover: false,
      spiderfyOnMaxZoom: true,
      zoomToBoundsOnClick: true,
      maxClusterRadius: 42,
      animate: false,
      iconCreateFunction(cluster) {
        const count = cluster.getChildCount()
        const content = document.createElement('span')
        const visible = document.createElement('span')
        visible.setAttribute('aria-hidden', 'true')
        visible.textContent = String(count)
        const label = document.createElement('span')
        label.className = 'sr-only'
        label.textContent = `${count} hlášení v této oblasti. Přiblížit nebo rozbalit.`
        content.append(visible, label)
        return L.divIcon({ html: content, className: 'report-map-cluster', iconSize: L.point(44, 44) })
      },
    })
    const markers = points.map(point => L.marker([point.latitude, point.longitude], {
      title: `Hlášení č. ${point.id}: ${point.description}`,
      alt: `Hlášení č. ${point.id}: ${point.description}`,
      riseOnHover: true,
    }).bindPopup(() => overviewPopup(point), { maxWidth: 260, minWidth: 200, maxHeight: 240, autoPanPadding: [18, 18] }).on('popupopen', event => {
      const close = event.popup.getElement()?.querySelector('.leaflet-popup-close-button')
      close?.setAttribute('aria-label', 'Zavřít náhled')
      close?.setAttribute('title', 'Zavřít náhled')
    }))
    group.addLayers(markers)
    state.map.addLayer(group)
    if (points.length) state.map.fitBounds(points.map(point => [point.latitude, point.longitude]), {
      padding: [35, 35], maxZoom: 16, animate: false,
    })
    root.dataset.loaded = 'true'
    root.dataset.pointCount = String(points.length)
  }).catch(() => {
    if (state.active) {
      state.map?.remove()
      state.map = null
      root.textContent = 'Mapu hlášení se nepodařilo načíst. Hlášení najdete v seznamu pod mapou.'
    }
  })
}
