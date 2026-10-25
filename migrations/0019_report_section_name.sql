-- Rename only the previous system title. Preserve custom labels and other links.
-- Advance the editor version so stale administration forms cannot overwrite it.
UPDATE navigation_items
SET label='Podněty a závady', version=version+1
WHERE path='/hlaseni-zavad' AND label='Hlášení závad';
