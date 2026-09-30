-- Ukázková data úřední desky. Vkládají se jen do prázdné desky (viz db.rs).
--
-- Datumy jsou schválně relativní k dnešku, ne pevné: ukázka pak dává smysl
-- i za rok a je na ní vidět celá škála stavů — čerstvě vyvěšeno, blíží se
-- lhůta, bez omezení, sejmuto s přílohami i sejmuto bez nich.

INSERT INTO notices
    (title, reference_number, category_id, issuer, description,
     published_on, withdraw_on, withdrawn_at, retain_attachments, status)
VALUES
    ('Veřejná vyhláška — opatření obecné povahy, stanovení přechodné úpravy provozu na silnici III/2809',
     'OUV-412/2026',
     (SELECT id FROM categories WHERE name = 'Veřejná vyhláška'),
     'Městský úřad Turnov, odbor dopravní',
     'Městský úřad Turnov, odbor dopravní, stanovuje přechodnou úpravu provozu na silnici III/2809 v úseku Vyskeř — Troskovice z důvodu opravy propustku.',
     ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (-4)), ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (+11)), NULL, TRUE, 'published'),

    ('Záměr obce pronajmout pozemek p. č. 412/3 v katastrálním území Vyskeř',
     'OUV-405/2026',
     (SELECT id FROM categories WHERE name = 'Záměr obce'),
     'Obec Vyskeř',
     'Obec Vyskeř zveřejňuje záměr pronajmout pozemek p. č. 412/3 o výměře 1 240 m². Nabídky lze podávat po celou dobu vyvěšení.',
     ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (-6)), ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (+9)), NULL, TRUE, 'published'),

    ('Oznámení o konání 5. zasedání Zastupitelstva obce Vyskeř',
     'OUV-398/2026',
     (SELECT id FROM categories WHERE name = 'Zastupitelstvo'),
     'Obec Vyskeř',
     'Zasedání se koná v zasedací místnosti obecního úřadu. Jednání je veřejné.',
     ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (-9)), ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (+3)), NULL, FALSE, 'published'),

    ('Rozpočtové opatření č. 4/2026',
     'OUV-391/2026',
     (SELECT id FROM categories WHERE name = 'Rozpočet'),
     'Obec Vyskeř',
     'Rozpočtové opatření schválené zastupitelstvem obce. Zveřejňuje se po dobu své platnosti.',
     ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (-16)), NULL, NULL, TRUE, 'published'),

    ('Oznámení o době a místě konání voleb do Poslanecké sněmovny',
     'OUV-377/2026',
     (SELECT id FROM categories WHERE name = 'Volby'),
     'Obec Vyskeř',
     'Volební místnost je v budově obecního úřadu.',
     ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (-20)), ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (-5)),
     to_char((CURRENT_TIMESTAMP AT TIME ZONE 'UTC') + INTERVAL '-5 days', 'YYYY-MM-DD"T"03:00:00"Z"'), TRUE, 'archived'),

    ('Dražební vyhláška — nedobrovolná dražba nemovité věci',
     'OUV-361/2026',
     (SELECT id FROM categories WHERE name = 'Dražba'),
     'Exekutorský úřad Semily',
     'Dražební vyhláška podle exekučního řádu.',
     ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (-32)), ((CURRENT_TIMESTAMP AT TIME ZONE 'Europe/Prague')::date + (-17)),
     to_char((CURRENT_TIMESTAMP AT TIME ZONE 'UTC') + INTERVAL '-17 days', 'YYYY-MM-DD"T"03:00:00"Z"'), FALSE, 'archived');

INSERT INTO attachments (notice_id, name, storage_path, content_type, size_bytes, sort_order, removed_at)
VALUES
    ((SELECT id FROM notices WHERE reference_number = 'OUV-412/2026'),
     'Opatření obecné povahy', 'ukazka/ouv-412-opatreni.pdf', 'application/pdf', 253952, 0, NULL),
    ((SELECT id FROM notices WHERE reference_number = 'OUV-412/2026'),
     'Situační výkres', 'ukazka/ouv-412-vykres.pdf', 'application/pdf', 1153434, 1, NULL),

    ((SELECT id FROM notices WHERE reference_number = 'OUV-405/2026'),
     'Záměr obce', 'ukazka/ouv-405-zamer.pdf', 'application/pdf', 98304, 0, NULL),
    ((SELECT id FROM notices WHERE reference_number = 'OUV-405/2026'),
     'Snímek katastrální mapy', 'ukazka/ouv-405-mapa.pdf', 'application/pdf', 1468006, 1, NULL),

    ((SELECT id FROM notices WHERE reference_number = 'OUV-398/2026'),
     'Pozvánka a program', 'ukazka/ouv-398-pozvanka.pdf', 'application/pdf', 114688, 0, NULL),

    ((SELECT id FROM notices WHERE reference_number = 'OUV-391/2026'),
     'Rozpočtové opatření', 'ukazka/ouv-391-opatreni.pdf', 'application/pdf', 188416, 0, NULL),

    ((SELECT id FROM notices WHERE reference_number = 'OUV-377/2026'),
     'Oznámení', 'ukazka/ouv-377-oznameni.pdf', 'application/pdf', 90112, 0, NULL),

    -- U dražební vyhlášky se přílohy v archivu neponechávají: řádek zůstává,
    -- ať je dohledatelné, co k vyvěšení patřilo, ale soubor je pryč.
    ((SELECT id FROM notices WHERE reference_number = 'OUV-361/2026'),
     'Dražební vyhláška', 'ukazka/ouv-361-vyhlaska.pdf', 'application/pdf', 204800, 0,
     to_char((CURRENT_TIMESTAMP AT TIME ZONE 'UTC') + INTERVAL '-17 days', 'YYYY-MM-DD"T"03:00:00"Z"'));
