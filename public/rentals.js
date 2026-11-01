const mounts = new WeakMap()

function mount(root) {
  unmount_rental(root)
  const controller = new AbortController()
  mounts.set(root, controller)
  return controller.signal
}

export function unmount_rental(root) {
  mounts.get(root)?.abort()
  mounts.delete(root)
}

function dateKey(date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}

function dateFromKey(value) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value)
  if (!match) return null
  const result = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]), 12)
  return dateKey(result) === value ? result : null
}

async function readJson(response) {
  const data = await response.json().catch(() => ({}))
  if (!response.ok) {
    const error = new Error(typeof data.error === 'string' ? data.error : 'Operaci se nepodařilo dokončit. Zkuste to prosím znovu.')
    error.status = response.status
    throw error
  }
  return data
}

function setMessage(element, message, error = false) {
  element.textContent = message
  element.classList.toggle('rental-error', error)
}

export function mount_rentals(root) {
  const signal = mount(root)
  const select = root.querySelector('#rental-asset')
  const fields = root.querySelector('#rental-fields')
  const form = root.querySelector('#rental-form')
  const start = root.querySelector('#rental-start')
  const end = root.querySelector('#rental-end')
  const message = root.querySelector('#rental-message')
  const calendarMessage = root.querySelector('#rental-calendar-message')
  const submit = root.querySelector('#rental-submit')
  const calendar = root.querySelector('#rental-calendar')
  const monthLabel = root.querySelector('#rental-month')
  const previous = root.querySelector('#rental-previous')
  const next = root.querySelector('#rental-next')
  const clear = root.querySelector('#rental-clear')
  const description = root.querySelector('#rental-asset-description')
  const prices = root.querySelector('#rental-prices')
  const eventType = root.querySelector('#rental-event-type')
  const modeHour = root.querySelector('#rental-mode-hour')
  const modeDay = root.querySelector('#rental-mode-day')
  const calendarHelp = root.querySelector('#rental-calendar-help')
  let selectedMode = 'hour'
  let dayAnchor = null
  let hourAnchor = null
  const startTime = root.querySelector('#rental-start-time')
  const endTime = root.querySelector('#rental-end-time')
  const timeFields = root.querySelector('#rental-time-fields')
  const endField = root.querySelector('#rental-end-field')
  const hoursPanel = root.querySelector('#rental-hours-panel')
  const hours = root.querySelector('#rental-hours')
  const hoursHelp = root.querySelector('#rental-hours-help')
  const summary = root.querySelector('#rental-selection-summary')
  const pickedPrice = root.querySelector('#rental-picked-price')
  const address = root.querySelector('#rental-address')
  const addressResults = root.querySelector('#rental-address-results')
  const addressStatus = root.querySelector('#rental-address-status')
  const tenantType = root.querySelector('#rental-tenant-type')
  const organizationFields = root.querySelector('#rental-organization-fields')
  const companyId = root.querySelector('#rental-company-id')
  const responsibleName = root.querySelector('#rental-responsible-name')
  const personAddress = root.querySelector('#rental-person-address')
  const dayFormatter = new Intl.DateTimeFormat('en', { timeZone: 'Europe/Prague', calendar: 'gregory', numberingSystem: 'latn', year: 'numeric', month: '2-digit', day: '2-digit' })
  const minuteFormatter = new Intl.DateTimeFormat('en-GB', { timeZone: 'Europe/Prague', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })
  function pragueDayKey(date) {
    const parts = Object.fromEntries(dayFormatter.formatToParts(date).map(part => [part.type, part.value]))
    return `${parts.year}-${parts.month}-${parts.day}`
  }
  let todayKey = pragueDayKey(new Date())
  let today = dateFromKey(todayKey)
  let clockSecond = -1
  let currentMinute = 0
  start.min = todayKey
  end.min = todayKey
  let month = new Date(today.getFullYear(), today.getMonth(), 1, 12)
  let assets = []
  let occupied = []
  let generation = 0
  let loading = true
  let submitting = false
  let addressTimer = null
  let addressTimeout = null
  let addressRequest = null
  let addressSequence = 0
  let addressSuggestions = []
  let selectedAddress = -1

  function cancelAddressSearch() {
    clearTimeout(addressTimer)
    clearTimeout(addressTimeout)
    addressRequest?.abort()
    addressRequest = null
    addressSequence += 1
  }

  function closeAddressSuggestions() {
    addressResults.hidden = true
    address.setAttribute('aria-expanded', 'false')
    address.removeAttribute('aria-activedescendant')
    selectedAddress = -1
    addressSuggestions = []
    addressResults.replaceChildren()
  }

  function selectAddress(index) {
    const suggestion = addressSuggestions[index]
    if (!suggestion) return
    cancelAddressSearch()
    address.value = suggestion.label
    closeAddressSuggestions()
    addressStatus.textContent = 'Adresa byla vybrána. Můžete ji upravit.'
    address.focus()
  }

  function highlightAddress(index) {
    selectedAddress = index
    for (const [position, option] of [...addressResults.children].entries()) {
      option.setAttribute('aria-selected', String(position === index))
    }
    const option = addressResults.children[index]
    if (option) {
      address.setAttribute('aria-activedescendant', option.id)
      option.scrollIntoView({ block: 'nearest' })
    }
  }

  function showAddressSuggestions(items) {
    addressSuggestions = items
    selectedAddress = -1
    addressResults.replaceChildren()
    for (const [index, item] of items.entries()) {
      const option = document.createElement('li')
      option.id = `rental-address-option-${index}`
      option.setAttribute('role', 'option')
      option.setAttribute('aria-selected', 'false')
      option.textContent = item.label
      option.addEventListener('pointerdown', event => event.preventDefault())
      option.addEventListener('click', () => selectAddress(index))
      addressResults.append(option)
    }
    addressResults.hidden = items.length === 0
    address.setAttribute('aria-expanded', String(items.length > 0))
    address.removeAttribute('aria-activedescendant')
    addressStatus.textContent = items.length
      ? `Nalezeno ${items.length} adres. Můžete zadat vlastní.`
      : 'Adresu jsme nenašli. Můžete ji zadat ručně.'
  }

  async function searchAddress(query, sequence) {
    if (signal.aborted || address.matches(':disabled') || document.activeElement !== address || sequence !== addressSequence) return
    const request = new AbortController()
    addressRequest = request
    addressTimeout = setTimeout(() => request.abort(), 6000)
    addressStatus.textContent = 'Vyhledávám adresy…'
    try {
      const response = await fetch(`/api/v1/report-addresses?q=${encodeURIComponent(query)}`, {
        credentials: 'same-origin', signal: request.signal,
      })
      if (!response.ok) throw new Error('Address search unavailable')
      const items = await response.json()
      if (signal.aborted || sequence !== addressSequence || address.matches(':disabled') || document.activeElement !== address || address.value.trim() !== query) return
      if (!Array.isArray(items)) throw new Error('Invalid address response')
      showAddressSuggestions(items.filter(item => typeof item?.label === 'string' && item.label.length <= 300).slice(0, 6))
    } catch (_) {
      if (!signal.aborted && sequence === addressSequence && !address.matches(':disabled') && document.activeElement === address) {
        closeAddressSuggestions()
        addressStatus.textContent = 'Našeptávání není dostupné. Adresu můžete zadat ručně.'
      }
    } finally {
      if (sequence === addressSequence) {
        clearTimeout(addressTimeout)
        addressRequest = null
      }
    }
  }

  function queueAddressSearch() {
    cancelAddressSearch()
    closeAddressSuggestions()
    addressStatus.textContent = ''
    if (address.matches(':disabled') || document.activeElement !== address) return
    const query = address.value.trim()
    if ([...query].length < 3) return
    if ([...query].length > 200) {
      addressStatus.textContent = 'Adresu lze zadat ručně. Našeptávání hledá kratší text.'
      return
    }
    const sequence = addressSequence
    addressTimer = setTimeout(() => searchAddress(query, sequence), 500)
  }

  address.addEventListener('input', queueAddressSearch, { signal })
  address.addEventListener('blur', () => {
    cancelAddressSearch()
    closeAddressSuggestions()
    if (addressStatus.textContent) addressStatus.textContent = 'Adresu můžete zadat ručně.'
  }, { signal })
  address.addEventListener('keydown', event => {
    if (event.key === 'Escape') {
      cancelAddressSearch()
      closeAddressSuggestions()
      addressStatus.textContent = ''
      return
    }
    if (addressResults.hidden) return
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      const count = addressSuggestions.length
      const next = selectedAddress < 0
        ? (event.key === 'ArrowDown' ? 0 : count - 1)
        : (selectedAddress + (event.key === 'ArrowDown' ? 1 : -1) + count) % count
      highlightAddress(next)
    } else if (event.key === 'Enter' && selectedAddress >= 0) {
      event.preventDefault()
      selectAddress(selectedAddress)
    }
  }, { signal })
  signal.addEventListener('abort', cancelAddressSearch, { once: true })

  const hourly = () => selectedMode === 'hour'
  const currentAsset = () => assets.find(item => String(item.id) === select.value)
  const groupLabel = () => eventType.value === 'nonprofit' ? 'Veřejně prospěšná nezisková akce' : 'Soukromá akce'
  const currentRates = () => currentAsset()?.rates?.[eventType.value] || {}
  const timeLabel = minute => `${String(Math.floor(minute / 60)).padStart(2, '0')}:00`
  const timeMinute = value => /^([01]\d|2[0-4]):00$/.test(value) ? Number(value.slice(0, 2)) * 60 : null
  const weekend = day => day.getDay() === 0 || day.getDay() === 6
  const displayDay = key => dateFromKey(key)?.toLocaleDateString('cs-CZ') || key
  const countDays = (first, last) => Math.round((Date.UTC(last.getFullYear(), last.getMonth(), last.getDate()) - Date.UTC(first.getFullYear(), first.getMonth(), first.getDate())) / 86400000) + 1

  function refreshRates() {
    const rates = currentRates()
    const oldMode = selectedMode
    const parts = []
    if (rates.weekend_day_price != null) parts.push(`Celý den: ${rates.weekend_day_price.toLocaleString('cs-CZ')} Kč`)
    if (rates.hourly_price != null) parts.push(`Hodina: ${rates.hourly_price.toLocaleString('cs-CZ')} Kč`)
    prices.textContent = parts.length
      ? `${groupLabel()}. ${parts.join(' · ')}`
      : 'Ceník pro tento typ akce zatím není nastavený. Obraťte se prosím na obecní úřad.'
    if (hourly() && rates.hourly_price == null && rates.weekend_day_price != null) selectedMode = 'day'
    else if (!hourly() && rates.weekend_day_price == null && rates.hourly_price != null) selectedMode = 'hour'
    if (oldMode !== selectedMode) {
      start.value = end.value = startTime.value = endTime.value = ''
    }
    dayAnchor = null
    hourAnchor = null
    if (!loading) { drawCalendar(); drawHours() }
  }

  function validateCompanyId() {
    companyId.setCustomValidity('')
    if (tenantType.value !== 'organization' || !companyId.value) return
    const value = companyId.value.trim()
    if (!/^[0-9]{8}$/.test(value)) {
      companyId.setCustomValidity('IČO musí mít 8 číslic.')
      return
    }
    const sum = [...value.slice(0, 7)].reduce((total, digit, index) => total + Number(digit) * (8 - index), 0)
    if (Number(value[7]) !== (11 - sum % 11) % 10) {
      companyId.setCustomValidity('Zkontrolujte IČO. Kontrolní číslice nesouhlasí.')
    }
  }

  function updateTenantLabels(event) {
    const organization = tenantType.value === 'organization'
    root.querySelector('#rental-name-label').textContent = organization ? 'Název firmy nebo organizace' : 'Jméno a příjmení'
    root.querySelector('#rental-name').autocomplete = organization ? 'organization' : 'name'
    organizationFields.hidden = !organization
    companyId.disabled = !organization
    companyId.required = organization
    responsibleName.disabled = !organization
    responsibleName.required = organization
    personAddress.hidden = organization
    address.disabled = organization
    address.required = !organization
    root.querySelector('#rental-identity-label').textContent = organization ? 'Kopie občanského průkazu odpovědné osoby' : 'Kopie občanského průkazu'
    root.querySelector('#rental-consent-label').textContent = organization
      ? 'Jako odpovědná osoba souhlasím s dočasným uložením kopie mého občanského průkazu pro účely tohoto pronájmu.'
      : 'Souhlasím s dočasným uložením kopie mého občanského průkazu pro účely tohoto pronájmu.'
    if (organization) {
      cancelAddressSearch()
      closeAddressSuggestions()
      addressStatus.textContent = ''
    }
    if (event?.type === 'change') {
      root.querySelector('#rental-identity').value = ''
      form.elements.consent.checked = false
    }
    validateCompanyId()
  }
  tenantType.addEventListener('change', updateTenantLabels, { signal })
  companyId.addEventListener('input', validateCompanyId, { signal })
  companyId.addEventListener('blur', () => { companyId.value = companyId.value.trim(); validateCompanyId() }, { signal })
  updateTenantLabels()
  eventType.addEventListener('change', () => {
    setMessage(message, '')
    setMessage(calendarMessage, '')
    refreshRates()
  }, { signal })

  function hourlyAllowed(key) {
    const day = dateFromKey(key)
    return Boolean(day && !weekend(day) && currentRates().hourly_price != null)
  }

  function refreshClock() {
    const second = Math.floor(Date.now() / 1000)
    if (second !== clockSecond) {
      clockSecond = second
      const now = new Date()
      todayKey = pragueDayKey(now)
      today = dateFromKey(todayKey)
      const parts = minuteFormatter.formatToParts(now)
      currentMinute = Number(parts.find(part => part.type === 'hour').value) * 60 + Number(parts.find(part => part.type === 'minute').value)
    }
    start.min = hourly() ? todayKey : dateKey(new Date(today.getFullYear(), today.getMonth(), today.getDate() + 1, 12))
    end.min = start.value && start.value >= start.min ? start.value : start.min
  }

  function hourPast(key, minute) {
    refreshClock()
    if (key < todayKey) return true
    if (key > todayKey) return false
    return minute <= currentMinute
  }

  function periodsFor(key) {
    return occupied.filter(item => item.start_date <= key && item.end_date >= key)
  }

  function overlappingHours(key, first, last) {
    return periodsFor(key).filter(item => {
      if (!item.start_time || !item.end_time) return true
      return timeMinute(item.start_time) < last && timeMinute(item.end_time) > first
    })
  }

  function freeHour(key, minute) {
    return Number.isInteger(minute) && minute >= 0 && minute < 1440 && minute % 60 === 0
      && hourlyAllowed(key) && !hourPast(key, minute) && overlappingHours(key, minute, minute + 60).length === 0
  }

  function freeHours(key, first, last) {
    if (first == null || last == null || first >= last || first < 0 || last > 1440) return false
    for (let minute = first; minute < last; minute += 60) if (!freeHour(key, minute)) return false
    return true
  }

  function freeDays(first, last) {
    refreshClock()
    const firstDay = dateFromKey(first)
    const lastDay = dateFromKey(last)
    return Boolean(firstDay && lastDay && first > todayKey && first <= last
      && currentRates().weekend_day_price != null && countDays(firstDay, lastDay) <= 28
      && !occupied.some(item => item.start_date <= last && item.end_date >= first))
  }

  function dayStatus(key) {
    const items = periodsFor(key)
    const full = items.find(item => !item.start_time || !item.end_time)
    const partial = !full && items.length > 0
    const anyFree = Array.from({ length: 24 }, (_, hour) => hour * 60).some(minute => freeHour(key, minute))
    return { items, full, partial, anyFree }
  }

  function selectedError() {
    refreshClock()
    const first = dateFromKey(start.value)
    const last = dateFromKey(end.value)
    if (!first || !last || first > last) return 'Vyberte začátek a konec pronájmu.'
    if (start.value < todayKey) return 'Začátek pronájmu nesmí být v minulosti.'
    if (hourly()) {
      if (!hourlyAllowed(start.value)) return 'Po hodinách lze tento prostor rezervovat pouze od pondělí do pátku, pokud je hodinový pronájem v nabídce.'
      if (start.value !== end.value) return 'Hodinová rezervace musí být v rámci jednoho dne.'
      const firstMinute = timeMinute(startTime.value)
      const lastMinute = timeMinute(endTime.value)
      if (firstMinute == null || lastMinute == null || firstMinute >= lastMinute) return 'Vyberte začátek a konec hodinové rezervace.'
      if (!freeHours(start.value, firstMinute, lastMinute)) return 'V tomto rozmezí už jsou obsazené nebo uplynulé hodiny. Vyberte souvislý volný čas.'
    } else {
      if (start.value <= todayKey) return 'Celodenní pronájem začíná o půlnoci. Vyberte některý z příštích dnů.'
      const days = countDays(first, last)
      if (days > 28) return 'Pronájem může trvat nejvýše 28 dní.'
      if (occupied.some(item => item.start_date <= end.value && item.end_date >= start.value)) return 'Část vybraného termínu je obsazená. Celý den lze pronajmout jen tehdy, je-li úplně volný. Vyberte jiný den nebo volné hodiny.'
      if (currentRates().weekend_day_price == null) return 'Cena za celý den pro tento typ akce zatím není nastavená. Obraťte se prosím na obecní úřad.'
    }
    return null
  }

  function updateSummary() {
    const asset = currentAsset()
    pickedPrice.hidden = true
    pickedPrice.textContent = ''
    submit.disabled = loading || submitting || Boolean(selectedError())
    if (!asset || !start.value || !end.value) {
      summary.classList.remove('rental-error')
      summary.textContent = 'Nejprve si vyberte termín. Platí se až po vyúčtování.'
      return
    }
    const error = selectedError()
    const needsHours = hourly() && !startTime.value && !endTime.value
    summary.classList.toggle('rental-error', Boolean(error) && !needsHours)
    if (needsHours) { summary.textContent = `${displayDay(start.value)}. Nyní vyberte volné hodiny.`; return }
    if (error) { summary.textContent = error; return }
    let price = 0
    let term
    if (hourly()) {
      const duration = (timeMinute(endTime.value) - timeMinute(startTime.value)) / 60
      price = duration * currentRates().hourly_price
      term = `${displayDay(start.value)}, ${startTime.value}–${endTime.value}`
    } else {
      const first = dateFromKey(start.value)
      const days = countDays(first, dateFromKey(end.value))
      price = days * currentRates().weekend_day_price
      term = start.value === end.value ? `${displayDay(start.value)}, celý den` : `${displayDay(start.value)} až ${displayDay(end.value)}`
    }
    summary.textContent = `${term}. Nájemné: ${price.toLocaleString('cs-CZ')} Kč (${groupLabel().toLocaleLowerCase('cs-CZ')}). Případné další náklady se vyúčtují po pronájmu.`
    pickedPrice.textContent = `${term} · ${price.toLocaleString('cs-CZ')} Kč`
    pickedPrice.hidden = false
  }

  function rovingFocus(container, attribute, preferred) {
    const buttons = [...container.querySelectorAll(`button[${attribute}]`)]
    const active = buttons.find(button => !button.disabled && button.getAttribute(attribute) === String(preferred))
      || buttons.find(button => !button.disabled)
    for (const button of buttons) button.tabIndex = button === active ? 0 : -1
    return active
  }

  function clearPreview(container) {
    for (const element of container.querySelectorAll('.rental-preview')) element.classList.remove('rental-preview')
  }

  function previewSelection(container, target) {
    clearPreview(container)
    const isCalendar = container === calendar
    const attribute = isCalendar ? 'data-date' : 'data-start-minute'
    const button = target.closest(`button[${attribute}]`)
    if (!button || button.disabled) return
    const anchor = isCalendar ? dayAnchor : hourAnchor
    if (anchor == null || (isCalendar && hourly())) return
    const value = isCalendar ? button.dataset.date : Number(button.dataset.startMinute)
    const first = value < anchor ? value : anchor
    const last = value > anchor ? value : anchor
    const free = isCalendar ? freeDays(first, last) : freeHours(start.value, first, last + 60)
    for (const candidate of container.querySelectorAll(`button[${attribute}]`)) {
      const key = isCalendar ? candidate.dataset.date : Number(candidate.dataset.startMinute)
      const preview = free ? key >= first && key <= last : key === value
      const element = isCalendar ? candidate.parentElement : candidate
      element.classList.toggle('rental-preview', preview && !candidate.disabled)
    }
  }

  function navigatePicker(event, container, attribute, columns) {
    const current = event.target.closest(`button[${attribute}]`)
    if (!current) return
    if (event.key === 'Escape') {
      event.preventDefault()
      if (container === calendar) clearSelection()
      else { startTime.value = endTime.value = ''; hourAnchor = null; drawHours(Number(current.dataset.startMinute)) }
      container.querySelector(`button[${attribute}="${current.getAttribute(attribute)}"]`)?.focus()
      return
    }
    const steps = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: columns, ArrowUp: -columns }
    if (!(event.key in steps) && event.key !== 'Home' && event.key !== 'End') return
    event.preventDefault()
    const buttons = [...container.querySelectorAll(`button[${attribute}]`)]
    let index = buttons.indexOf(current)
    const step = steps[event.key] || (event.key === 'Home' ? 1 : -1)
    index = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : index + step
    while (index >= 0 && index < buttons.length && buttons[index].disabled) index += step
    if (index < 0 || index >= buttons.length) return
    rovingFocus(container, attribute, buttons[index].getAttribute(attribute))
    buttons[index].focus()
  }

  function drawHours(focusMinute) {
    refreshClock()
    hours.replaceChildren()
    startTime.disabled = loading || submitting || !hourly()
    endTime.disabled = loading || submitting || !hourly()
    startTime.required = hourly()
    endTime.required = hourly()
    timeFields.hidden = !hourly() || !start.value
    hoursPanel.hidden = !hourly() || !start.value
    endField.hidden = hourly()
    start.disabled = loading || submitting
    end.disabled = loading || submitting
    root.querySelector('#rental-start-label').textContent = hourly() ? 'Datum' : 'Od'
    const selectedDay = dateFromKey(start.value)
    end.max = selectedDay ? dateKey(new Date(selectedDay.getFullYear(), selectedDay.getMonth(), selectedDay.getDate() + 27, 12)) : ''
    if (!hourly()) { updateSummary(); return }
    let firstValue = startTime.value
    let lastValue = endTime.value
    if (firstValue && hourPast(start.value, timeMinute(firstValue))) {
      firstValue = lastValue = ''
      hourAnchor = null
      setMessage(calendarMessage, 'Vybraný čas už začal. Vyberte prosím nový termín.')
    }
    startTime.replaceChildren(new Option('Vyberte začátek', ''))
    endTime.replaceChildren(new Option('Vyberte konec', ''))
    if (!dateFromKey(start.value)) {
      hoursHelp.textContent = 'Nejprve vyberte den v kalendáři.'
      updateSummary()
      return
    }
    if (!hourlyAllowed(start.value)) {
      hoursHelp.textContent = 'Hodinová cena pro tento typ akce zatím není nastavená. Obraťte se prosím na obecní úřad.'
      updateSummary()
      return
    }
    hoursHelp.textContent = hourAnchor != null
      ? 'Pro delší pronájem klikněte na poslední hodinu. Kliknutím na vybranou hodinu výběr zrušíte.'
      : firstValue ? 'Vybráno. Kliknutím mimo výběr začnete nový, kliknutím do výběru jej zrušíte.'
      : 'Klikněte na první a pak na poslední hodinu. Pro jednu hodinu stačí jeden klik.'
    for (let minute = 0; minute < 1440; minute += 60) {
      const option = new Option(timeLabel(minute), timeLabel(minute))
      option.disabled = !freeHour(start.value, minute)
      startTime.append(option)
    }
    startTime.value = firstValue
    const firstMinute = timeMinute(firstValue)
    for (let minute = 60; minute <= 1440; minute += 60) {
      const option = new Option(timeLabel(minute), timeLabel(minute))
      option.disabled = !freeHours(start.value, firstMinute, minute)
      endTime.append(option)
    }
    endTime.value = lastValue
    const lastMinute = timeMinute(lastValue)
    for (let minute = 0; minute < 1440; minute += 60) {
      const collisions = overlappingHours(start.value, minute, minute + 60)
      const past = hourPast(start.value, minute)
      const booked = collisions.some(item => item.status !== 'pending')
      const pending = !booked && collisions.length > 0
      const selected = firstMinute != null && lastMinute != null && firstMinute <= minute && minute < lastMinute
      const status = past ? 'uplynulý čas' : booked ? 'obsazeno' : pending ? 'žádost čeká na schválení' : 'volno'
      const button = document.createElement('button')
      button.type = 'button'
      button.dataset.startMinute = String(minute)
      button.dataset.endMinute = String(minute + 60)
      button.textContent = `${timeLabel(minute)}–${timeLabel(minute + 60)}`
      button.setAttribute('aria-label', `${button.textContent}, ${status}`)
      button.setAttribute('aria-pressed', selected ? 'true' : 'false')
      button.title = status
      button.disabled = loading || submitting || past || collisions.length > 0
      button.classList.toggle('rental-pending', pending)
      button.classList.toggle('rental-booked', booked)
      button.classList.toggle('rental-selected', selected && !past && collisions.length === 0)
      hours.append(button)
    }
    const active = rovingFocus(hours, 'data-start-minute', focusMinute ?? firstMinute)
    if (focusMinute != null) (active || start).focus({ preventScroll: true })
    updateSummary()
  }

  function selectDay(day) {
    const key = dateKey(day)
    refreshClock()
    if (key < todayKey || ((!hourly() || weekend(day)) && key === todayKey)) {
      setMessage(calendarMessage, 'Tento den už začal. Vyberte prosím budoucí termín.')
      drawCalendar()
      drawHours()
      return
    }
    if (start.value && start.value <= key && key <= end.value) {
      clearSelection()
      calendar.querySelector(`[data-date="${key}"]`)?.focus()
      return
    }
    if (weekend(day)) selectedMode = 'day'
    let first = key
    let last = key
    let notice = ''
    if (!hourly() && dayAnchor != null) {
      first = key < dayAnchor ? key : dayAnchor
      last = key > dayAnchor ? key : dayAnchor
      if (!freeDays(first, last)) {
        notice = countDays(dateFromKey(first), dateFromKey(last)) > 28
          ? 'Pronájem může trvat nejvýše 28 dní. Vybrali jsme nový začátek.'
          : 'Mezi těmito dny je obsazeno. Vybrali jsme nový začátek.'
        first = last = key
        dayAnchor = key
      } else dayAnchor = null
    } else dayAnchor = hourly() ? null : key
    start.value = first
    end.value = last
    hourAnchor = null
    startTime.value = ''
    endTime.value = ''
    setMessage(message, '')
    setMessage(calendarMessage, notice || (dayStatus(key).partial && hourly() ? 'Část tohoto dne je obsazená. Vyberte si z volných hodin.' : ''))
    drawCalendar(key)
    drawHours()
  }

  function drawCalendar(focusDate) {
    refreshClock()
    modeHour.disabled = loading || submitting || currentRates().hourly_price == null
    modeDay.disabled = loading || submitting || currentRates().weekend_day_price == null
    modeHour.setAttribute('aria-pressed', String(hourly()))
    modeDay.setAttribute('aria-pressed', String(!hourly()))
    calendarHelp.textContent = hourly()
      ? 'Vyberte den. Opakovaným kliknutím jej zrušíte. O víkendu se vybere celý den.'
      : dayAnchor != null ? 'Pro více dnů klikněte na poslední den. Opakovaným kliknutím výběr zrušíte.'
      : start.value ? 'Kliknutím mimo výběr začnete nový, kliknutím do výběru jej zrušíte.'
      : 'Klikněte na první a pak na poslední den. Pro jeden den stačí jeden klik.'
    monthLabel.textContent = new Intl.DateTimeFormat('cs-CZ', { month: 'long', year: 'numeric' }).format(month)
    const table = document.createElement('table')
    table.className = 'rental-calendar-table'
    table.setAttribute('aria-label', `Dostupnost ${monthLabel.textContent}`)
    const head = table.createTHead().insertRow()
    for (const day of ['Po', 'Út', 'St', 'Čt', 'Pá', 'So', 'Ne']) {
      const th = document.createElement('th')
      th.scope = 'col'
      th.textContent = day
      head.append(th)
    }
    const body = table.createTBody()
    const first = new Date(month.getFullYear(), month.getMonth(), 1, 12)
    const offset = (first.getDay() + 6) % 7
    const count = new Date(month.getFullYear(), month.getMonth() + 1, 0, 12).getDate()
    let row
    for (let index = 0; index < offset + count; index++) {
      if (index % 7 === 0) row = body.insertRow()
      const cell = row.insertCell()
      if (index < offset) continue
      const day = index - offset + 1
      const date = new Date(month.getFullYear(), month.getMonth(), day, 12)
      const key = dateKey(date)
      const state = dayStatus(key)
      const isWeekend = weekend(date)
      const usesDay = !hourly() || isWeekend
      const past = key < todayKey || (usesDay && key === todayKey)
      const partialPending = state.partial && !isWeekend && state.items.some(item => item.status === 'pending')
      const onlyPending = partialPending && state.items.every(item => item.status === 'pending')
      const partialStatus = `${onlyPending ? 'část dne čeká na schválení' : 'část dne obsazena'}${partialPending && !onlyPending ? ', některé žádosti čekají na schválení' : ''}${state.anyFree ? ', dostupné hodiny' : ', žádné volné hodiny'}${usesDay ? ', celý den nelze rezervovat' : ''}`
      const dominant = state.full || (isWeekend ? state.items.find(item => item.status !== 'pending') || state.items[0] : null)
      const pending = dominant?.status === 'pending'
      const booked = Boolean(dominant) && !pending
      const offered = usesDay ? currentRates().weekend_day_price != null : currentRates().hourly_price != null
      const unavailable = !offered || (usesDay ? state.items.length > 0 : !state.anyFree)
      const selected = start.value && end.value && start.value <= key && key <= end.value
      const status = past ? 'uplynulé datum' : pending ? 'žádost čeká na schválení' : booked ? 'obsazeno' : !offered ? 'cena pro tento typ akce není nastavena' : state.partial ? partialStatus : usesDay ? 'volno, celý den' : !state.anyFree ? 'žádné volné hodiny' : 'volno po hodinách'
      cell.classList.toggle('rental-past', past)
      cell.classList.toggle('rental-pending', pending)
      cell.classList.toggle('rental-booked', booked)
      cell.classList.toggle('rental-partial', state.partial && !isWeekend)
      cell.classList.toggle('rental-partial-pending', onlyPending)
      cell.classList.toggle('rental-partial-mixed', partialPending && !onlyPending)
      cell.classList.toggle('rental-selected', Boolean(selected) && !unavailable && !past)
      const label = `${day}. ${monthLabel.textContent}, ${status}`
      cell.setAttribute('aria-label', label)
      const button = document.createElement('button')
      button.type = 'button'
      button.dataset.date = key
      button.textContent = String(day)
      button.setAttribute('aria-label', label)
      button.setAttribute('aria-pressed', selected && !unavailable && !past ? 'true' : 'false')
      if (key === todayKey) button.setAttribute('aria-current', 'date')
      button.disabled = loading || submitting || past || unavailable
      button.title = status
      cell.append(button)
    }
    calendar.replaceChildren(table)
    const active = rovingFocus(calendar, 'data-date', focusDate || start.value)
    if (focusDate) (active || next).focus({ preventScroll: true })
    previous.disabled = loading || submitting || month <= new Date(today.getFullYear(), today.getMonth(), 1, 12)
    next.disabled = loading || submitting
    clear.disabled = submitting || (!start.value && !end.value)
    updateSummary()
  }

  async function loadAvailability() {
    const asset = currentAsset()
    if (!asset) return
    const current = ++generation
    loading = true
    fields.disabled = true
    start.disabled = end.disabled = modeHour.disabled = modeDay.disabled = true
    startTime.disabled = endTime.disabled = true
    eventType.disabled = true
    previous.disabled = true
    next.disabled = true
    description.textContent = asset.description || ''
    refreshRates()
    calendar.textContent = 'Načítám dostupnost…'
    try {
      const items = await readJson(await fetch(`/api/v1/rental-assets/${asset.id}/availability`, { signal }))
      if (current !== generation || signal.aborted) return
      if (!Array.isArray(items)) throw new Error('Dostupnost se nepodařilo načíst.')
      occupied = items
      loading = false
      fields.disabled = submitting
      eventType.disabled = submitting
      drawCalendar()
      drawHours()
    } catch (error) {
      if (signal.aborted || current !== generation) return
      calendar.textContent = 'Dostupnost se nepodařilo načíst. Zkuste stránku obnovit.'
      setMessage(message, error.message, true)
    }
  }

  function clearSelection() {
    dayAnchor = hourAnchor = null
    start.value = ''
    end.value = ''
    startTime.value = ''
    endTime.value = ''
    setMessage(message, '')
    setMessage(calendarMessage, '')
    drawCalendar()
    drawHours()
  }

  select.addEventListener('change', () => { clearSelection(); loadAvailability() }, { signal })
  function changeMode(mode) {
    if (selectedMode === mode || loading) return
    selectedMode = mode
    dayAnchor = hourAnchor = null
    startTime.value = endTime.value = ''
    if (start.value && hourly() && weekend(dateFromKey(start.value))) start.value = end.value = ''
    else if (hourly()) end.value = start.value
    else {
      refreshClock()
      if (start.value && start.value <= todayKey) start.value = end.value = ''
    }
    setMessage(calendarMessage, '')
    setMessage(message, '')
    drawCalendar()
    drawHours()
  }
  modeHour.addEventListener('click', () => changeMode('hour'), { signal })
  modeDay.addEventListener('click', () => changeMode('day'), { signal })
  for (const container of [calendar, hours]) {
    container.addEventListener('pointerover', event => previewSelection(container, event.target), { signal })
    container.addEventListener('pointerleave', () => clearPreview(container), { signal })
    container.addEventListener('focusin', event => {
      const attribute = container === calendar ? 'data-date' : 'data-start-minute'
      const button = event.target.closest(`button[${attribute}]`)
      if (button) rovingFocus(container, attribute, button.getAttribute(attribute))
      previewSelection(container, event.target)
    }, { signal })
    container.addEventListener('focusout', event => {
      if (!container.contains(event.relatedTarget)) clearPreview(container)
    }, { signal })
    container.addEventListener('keydown', event => {
      const isCalendar = container === calendar
      const columns = isCalendar ? 7 : getComputedStyle(hours).gridTemplateColumns.split(' ').length
      navigatePicker(event, container, isCalendar ? 'data-date' : 'data-start-minute', columns)
    }, { signal })
  }
  calendar.addEventListener('click', event => {
    const button = event.target.closest('button[data-date]')
    if (!button || button.disabled || loading) return
    const day = dateFromKey(button.dataset.date)
    if (day) selectDay(day)
  }, { signal })
  hours.addEventListener('click', event => {
    const button = event.target.closest('button[data-start-minute]')
    if (!button || button.disabled || loading) return
    const minute = Number(button.dataset.startMinute)
    if (!freeHour(start.value, minute)) {
      setMessage(calendarMessage, 'Tato hodina už není dostupná. Vyberte prosím jiný čas.')
      drawHours(minute)
      return
    }
    const currentFirst = timeMinute(startTime.value)
    const currentLast = timeMinute(endTime.value)
    if (currentFirst != null && currentLast != null && minute >= currentFirst && minute < currentLast) {
      startTime.value = endTime.value = ''
      hourAnchor = null
      setMessage(calendarMessage, '')
      drawHours(minute)
      return
    }
    let first = hourAnchor == null ? minute : Math.min(hourAnchor, minute)
    let last = hourAnchor == null ? minute + 60 : Math.max(hourAnchor, minute) + 60
    let notice = ''
    if (!freeHours(start.value, first, last)) {
      first = minute
      last = minute + 60
      hourAnchor = minute
      notice = 'Mezi těmito hodinami je obsazeno. Vybrali jsme nový začátek.'
    } else hourAnchor = hourAnchor == null ? minute : null
    startTime.value = timeLabel(first)
    endTime.value = timeLabel(last)
    setMessage(message, '')
    setMessage(calendarMessage, notice)
    drawHours(minute)
  }, { signal })
  previous.addEventListener('click', () => { month = new Date(month.getFullYear(), month.getMonth() - 1, 1, 12); drawCalendar() }, { signal })
  next.addEventListener('click', () => { month = new Date(month.getFullYear(), month.getMonth() + 1, 1, 12); drawCalendar() }, { signal })
  for (const field of [start, end]) field.addEventListener('input', () => {
    dayAnchor = hourAnchor = null
    if (field === start) {
      const day = dateFromKey(start.value)
      if (day && weekend(day)) selectedMode = 'day'
      end.value = start.value
      startTime.value = ''
      endTime.value = ''
    }
    const selectedDay = dateFromKey(start.value)
    if (selectedDay) month = new Date(selectedDay.getFullYear(), selectedDay.getMonth(), 1, 12)
    setMessage(message, '')
    setMessage(calendarMessage, '')
    drawCalendar()
    drawHours()
  }, { signal })
  startTime.addEventListener('change', () => {
    hourAnchor = null
    setMessage(calendarMessage, '')
    const minute = timeMinute(startTime.value)
    endTime.value = minute != null && freeHour(start.value, minute) ? timeLabel(minute + 60) : ''
    drawHours()
  }, { signal })
  endTime.addEventListener('change', () => { hourAnchor = null; setMessage(calendarMessage, ''); drawHours() }, { signal })
  clear.addEventListener('click', () => {
    clearSelection()
    calendar.querySelector('button[tabindex="0"]')?.focus()
  }, { signal })
  function refreshPickerClock() {
    if (loading || submitting || document.hidden) return
    // Do not rebuild a native time menu while the visitor is choosing an option.
    if (document.activeElement === startTime || document.activeElement === endTime) {
      refreshClock()
      for (const option of startTime.options) {
        if (option.value) option.disabled = !freeHour(start.value, timeMinute(option.value))
      }
      const firstMinute = timeMinute(startTime.value)
      for (const option of endTime.options) {
        if (option.value) option.disabled = !freeHours(start.value, firstMinute, timeMinute(option.value))
      }
      updateSummary()
      return
    }
    const calendarFocus = calendar.contains(document.activeElement) ? document.activeElement.dataset.date : null
    const hourFocus = hours.contains(document.activeElement) ? Number(document.activeElement.dataset.startMinute) : null
    drawCalendar(calendarFocus)
    drawHours(hourFocus)
  }
  const pickerClock = setInterval(refreshPickerClock, 30_000)
  signal.addEventListener('abort', () => clearInterval(pickerClock), { once: true })
  window.addEventListener('focus', refreshPickerClock, { signal })
  document.addEventListener('visibilitychange', refreshPickerClock, { signal })
  form.addEventListener('submit', async event => {
    event.preventDefault()
    if (loading || submitting) return
    const error = selectedError()
    if (error) return setMessage(message, error, true)
    const identity = root.querySelector('#rental-identity').files[0]
    if (identity && identity.size > 10 * 1024 * 1024) return setMessage(message, 'Kopie občanského průkazu může mít nejvýše 10 MB.', true)
    const body = new FormData(form)
    if (tenantType.value === 'organization') body.delete('address')
    else { body.delete('organization_ico'); body.delete('responsible_name') }
    body.set('asset_id', select.value)
    body.set('event_type', eventType.value)
    if (!hourly()) { body.delete('start_time'); body.delete('end_time') }
    submitting = true
    fields.disabled = select.disabled = eventType.disabled = true
    cancelAddressSearch()
    closeAddressSuggestions()
    drawCalendar()
    drawHours()
    setMessage(message, 'Odesílám žádost…')
    try {
      await readJson(await fetch('/api/v1/rentals', { method: 'POST', body, signal }))
      form.reset()
      setMessage(calendarMessage, '')
      selectedMode = 'hour'
      dayAnchor = hourAnchor = null
      eventType.value = 'standard'
      updateTenantLabels()
      cancelAddressSearch()
      closeAddressSuggestions()
      addressStatus.textContent = ''
      await loadAvailability()
      setMessage(message, 'Žádost byla odeslána. O dalším postupu vás bude obec informovat.')
    } catch (error) {
      if (!signal.aborted) {
        if (error.status === 409) await loadAvailability()
        setMessage(message, error.message, true)
      }
    } finally {
      submitting = false
      fields.disabled = select.disabled = eventType.disabled = loading
      drawCalendar()
      drawHours()
    }
  }, { signal })

  fetch('/api/v1/rental-assets', { signal }).then(readJson).then(items => {
    if (!Array.isArray(items)) throw new Error('Nabídku se nepodařilo načíst.')
    assets = items
    select.replaceChildren()
    if (!items.length) {
      const option = document.createElement('option')
      option.textContent = 'Momentálně není nic k pronájmu'
      select.append(option)
      calendar.textContent = 'Aktuálně nejsou vypsány žádné pronájmy.'
      return
    }
    for (const asset of items) {
      const option = document.createElement('option')
      option.value = String(asset.id)
      option.textContent = asset.name
      select.append(option)
    }
    select.disabled = false
    loadAvailability()
  }).catch(error => {
    if (signal.aborted) return
    select.replaceChildren(new Option('Nabídku se nepodařilo načíst'))
    calendar.textContent = 'Zkuste stránku obnovit nebo se obraťte na obecní úřad.'
    setMessage(message, error.message, true)
  })
}

export function mount_rental_meter(root) {
  const signal = mount(root)
  const info = root.querySelector('#meter-info')
  const form = root.querySelector('#meter-form')
  const fields = root.querySelector('#meter-fields')
  const submit = root.querySelector('#meter-submit')
  const message = root.querySelector('#meter-message')
  let ready = false
  let saving = false
  const cards = ['before', 'after'].map(kind => {
    const card = root.querySelector(`[data-meter-card="${kind}"]`)
    return {
      kind, card, input: root.querySelector(`#meter-${kind}`),
      status: card.querySelector('[data-meter-status]'),
      preview: card.querySelector('[data-meter-preview]'),
      note: card.querySelector('[data-meter-preview-note]'),
      clear: card.querySelector('[data-meter-clear]'),
      uploaded: false, savedUrl: null, selectedUrl: null, request: null, image: null,
    }
  })
  const token = location.pathname.split('/').filter(Boolean).pop()
  if (!token || !/^[a-f0-9]{64}$/.test(token)) {
    info.textContent = 'Odkaz je neplatný. Obraťte se na obecní úřad.'
    return
  }
  const url = `/api/v1/rental-access/${encodeURIComponent(token)}`
  const fetchOptions = { signal, cache: 'no-store', referrerPolicy: 'no-referrer' }

  function validatePhoto(file) {
    if (!file) return ''
    if (file.size > 10 * 1024 * 1024) return 'Každá fotografie může mít nejvýše 10 MB.'
    if (!['image/jpeg', 'image/png', 'image/webp'].includes(file.type)) return 'Vyberte fotografii ve formátu JPG, PNG nebo WebP.'
    return ''
  }

  function revoke(card, key) {
    if (card[key]) URL.revokeObjectURL(card[key])
    card[key] = null
  }

  function updateControls() {
    const selected = cards.map(card => card.input.files[0]).filter(Boolean)
    fields.disabled = !ready || saving
    submit.disabled = !ready || saving || selected.length === 0 || selected.some(file => validatePhoto(file))
  }

  function renderCard(card) {
    const file = card.input.files[0]
    const selected = !!file
    card.status.textContent = selected ? 'Vybráno, zatím neuloženo' : card.uploaded ? 'Nahráno' : 'Zatím nenahráno'
    card.status.classList.toggle('is-uploaded', !selected && card.uploaded)
    card.status.classList.toggle('is-selected', selected)
    card.clear.hidden = !selected
    const src = selected ? card.selectedUrl : card.savedUrl
    if (card.image?.getAttribute('src') === src && src) return
    card.image?.removeAttribute('src')
    card.image = null
    card.preview.replaceChildren()
    if (src) {
      const image = document.createElement('img')
      image.alt = card.kind === 'before' ? 'Fotografie elektroměru před pronájmem' : 'Fotografie elektroměru po pronájmu'
      image.draggable = false
      image.addEventListener('error', () => {
        if (card.image !== image) return
        image.removeAttribute('src')
        image.remove()
        card.image = null
        card.note.textContent = selected
          ? 'Náhled nelze zobrazit. Zkontrolujte vybraný soubor.'
          : 'Náhled se nepodařilo zobrazit. Fotografie je uložená.'
      }, { once: true, signal })
      image.src = src
      card.image = image
      card.preview.append(image)
    } else {
      const placeholder = document.createElement('p')
      placeholder.textContent = selected ? 'Náhled vybraného souboru není dostupný.'
        : card.uploaded ? 'Načítám uloženou fotografii…'
          : card.kind === 'before' ? 'Fotografie elektroměru při převzetí' : 'Fotografie elektroměru při vrácení'
      card.preview.append(placeholder)
    }
  }

  async function loadPhoto(card) {
    card.request?.abort()
    if (!card.uploaded) {
      card.request = null
      revoke(card, 'savedUrl')
      renderCard(card)
      return
    }
    const request = new AbortController()
    card.request = request
    try {
      const response = await fetch(`${url}/photos/${card.kind}`, { ...fetchOptions, signal: request.signal })
      if (!response.ok) throw new Error('Photo preview unavailable')
      const blob = await response.blob()
      if (signal.aborted || request.signal.aborted || card.request !== request) return
      revoke(card, 'savedUrl')
      card.savedUrl = URL.createObjectURL(blob)
      if (!card.input.files.length) card.note.textContent = ''
      renderCard(card)
    } catch (_) {
      if (!signal.aborted && !request.signal.aborted && card.request === request && !card.input.files.length) {
        card.note.textContent = 'Náhled se nepodařilo načíst. Fotografie je uložená, stránku můžete obnovit.'
        card.preview.replaceChildren()
        card.image = null
      }
    } finally {
      if (card.request === request) card.request = null
    }
  }

  function describe(data) {
    const first = dateFromKey(data.start_date)?.toLocaleDateString('cs-CZ') || data.start_date
    const last = dateFromKey(data.end_date)?.toLocaleDateString('cs-CZ') || data.end_date
    const period = data.start_time && data.end_time ? `${first}, ${data.start_time}–${data.end_time}` : `${first} až ${last}`
    info.textContent = `${data.asset_name}, ${period}`
    for (const card of cards) {
      card.uploaded = data[`${card.kind}_uploaded`] === true
      renderCard(card)
    }
  }

  for (const card of cards) {
    card.input.addEventListener('change', () => {
      if (saving) return
      revoke(card, 'selectedUrl')
      const file = card.input.files[0]
      const failure = validatePhoto(file)
      if (file && !failure) card.selectedUrl = URL.createObjectURL(file)
      card.note.textContent = ''
      setMessage(message, failure, !!failure)
      renderCard(card)
      updateControls()
    }, { signal })
    card.clear.addEventListener('click', () => {
      if (saving) return
      card.input.value = ''
      revoke(card, 'selectedUrl')
      card.note.textContent = ''
      setMessage(message, '')
      renderCard(card)
      updateControls()
    }, { signal })
  }
  signal.addEventListener('abort', () => {
    for (const card of cards) {
      card.request?.abort()
      card.image?.removeAttribute('src')
      card.preview.replaceChildren()
      card.image = null
      card.input.value = ''
      revoke(card, 'selectedUrl')
      revoke(card, 'savedUrl')
    }
  }, { once: true })

  fetch(url, fetchOptions).then(readJson).then(data => {
    if (signal.aborted) return
    describe(data)
    ready = true
    updateControls()
    for (const card of cards) void loadPhoto(card)
  }).catch(error => {
    if (!signal.aborted) info.textContent = error.message
  })
  form.addEventListener('submit', async event => {
    event.preventDefault()
    if (!ready || saving || signal.aborted) return
    const selected = cards.filter(card => card.input.files.length)
    if (!selected.length) return setMessage(message, 'Nejprve vyberte alespoň jednu fotografii.', true)
    for (const card of selected) {
      const failure = validatePhoto(card.input.files[0])
      if (failure) return setMessage(message, failure, true)
    }
    const body = new FormData()
    for (const card of selected) body.append(card.kind, card.input.files[0])
    saving = true
    updateControls()
    setMessage(message, 'Ukládám fotografie…')
    try {
      const data = await readJson(await fetch(`${url}/photos`, { ...fetchOptions, method: 'POST', body }))
      if (signal.aborted) return
      for (const card of selected) {
        card.input.value = ''
        revoke(card, 'selectedUrl')
        revoke(card, 'savedUrl')
        card.note.textContent = ''
      }
      describe(data)
      setMessage(message, selected.length === 1 ? 'Fotografie byla uložena.' : 'Obě fotografie byly uloženy.')
      for (const card of cards) void loadPhoto(card)
    } catch (error) {
      if (!signal.aborted) setMessage(message, error.message, true)
    } finally {
      saving = false
      if (!signal.aborted) updateControls()
    }
  }, { signal })
}
