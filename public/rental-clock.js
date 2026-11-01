const prague = new Intl.DateTimeFormat('en', {
  timeZone: 'Europe/Prague', calendar: 'gregory', numberingSystem: 'latn',
  year: 'numeric', month: '2-digit', day: '2-digit',
  hour: '2-digit', minute: '2-digit', hourCycle: 'h23',
})

export function rental_prague_clock() {
  const parts = Object.fromEntries(prague.formatToParts(new Date()).map(part => [part.type, part.value]))
  return `${parts.year}-${parts.month}-${parts.day}T${parts.hour}:${parts.minute}`
}
