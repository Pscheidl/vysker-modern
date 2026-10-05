# Provoz testovacího nasazení

Tento postup používá připravené Compose soubory. Zabezpečení nového Ubuntu VPS
a předání SSH přístupu popisuje [plán nasazení](vps-test-deployment.md).
Příkazy níže se spouštějí na VPS v `/srv/obecni-web-test`, pokud není uvedeno jinak.
Neobsahují skutečné přihlašovací údaje a nepotvrzují stav konkrétního serveru.

## Obraz a konfigurace

Rust a WebAssembly sestavit na vývojovém počítači. Na VPS přenést hotový obraz
pomocí `docker save`, SSH a `docker load`. Alternativně lze přenést úzký balík
s hotovými binárními soubory a sestavit runtime pomocí `Dockerfile.prebuilt`.
Ten pouze instaluje provozní závislosti a kopíruje hotové soubory, Rust nekompiluje.
Balík vytvořit ze stejného sestavení serveru i klienta. Na vývojovém počítači
v připraveném Rust prostředí a kořeni projektu:

```bash
CARGO_NET_OFFLINE=true cargo leptos build --release \
  --lib-cargo-args=--locked --bin-cargo-args=--locked
CARGO_NET_OFFLINE=true cargo build --locked --release --bin obec-admin
release_parent=$(mktemp -d /tmp/obecni-release.XXXXXX)
python3 scripts/package-prebuilt.py "$release_parent/context"
tar -C "$release_parent/context" -czf "$release_parent/release.tar.gz" .
scp "$release_parent/release.tar.gz" obecni-test:release.tar.gz
```

`CARGO_NET_OFFLINE=true` použít při naplněné místní cache, jinak tuto proměnnou
vynechat. Výstupní složka balíku musí být nová a mimo checkout. Skript ověří
kompatibilitu glibc, přibalí pouze potřebné soubory a vypíše tag obrazu odvozený
z jejich obsahu. Manifest také obsahuje výchozí commit a stav pracovního stromu.
Na VPS archiv rozbalit do samostatné složky a v ní sestavit pouze runtime:

```bash
release_image=$(python3 -c 'import json; print(json.load(open("release-manifest.json"))["image_tag"])')
sudo docker build --tag "$release_image" .
```

`Dockerfile.prebuilt` má základ PostgreSQL 18 připnutý digestem. Výsledný tag
zapsat do `WEB_IMAGE` v `deploy/.env`. Compose vyžaduje již dostupný místní obraz
a nikdy ho samo nesestavuje ani nestahuje.

Přenést odpovídající Compose soubory, `deploy/init-postgres.sh`,
`deploy/Caddyfile.staging` a šablony systemd. Zkopírovat
`deploy/staging.env.example` do privátního `deploy/.env` a doplnit `WEB_IMAGE`.
Pro HTTPS také `DOMAIN` a `TLS_EMAIL`. Do Gitu ani výpisu konfigurace nepatří hesla.

Nová náhodná databázová hesla uložit pod `deploy/secrets/`. Adresář vlastní
root s právy `0700`. Compose připojuje jednotlivé soubory do kontejnerů.

| Soubor | Obsah | Kdo ho musí v kontejneru přečíst |
| --- | --- | --- |
| `postgres-password.txt` | Heslo správce PostgreSQL | root při startu obrazu |
| `database-password.txt` | Heslo nové aplikační role `vysker` | účet `postgres` při prvním vytvoření databáze |
| `database-url.txt` | `postgresql://vysker:HESLO@postgres:5432/vysker` | UID 10001, aplikace a monitor |
| `smtp-password.txt` | Heslo SMTP, pouze při výslovném zapnutí skutečné pošty | UID 10001 |

Soubory mají práva `0400` a odpovídajícího vlastníka. UID databázového účtu
ověřit v použitém obrazu, například `docker run --rm --entrypoint id
postgres:18-bookworm -u postgres`. Pro heslo v URL použít náhodné znaky
bez nutnosti kódování, například hexadecimální, nebo ho správně percent-enkódovat.
Změna souboru hesla po inicializaci svazku sama nezmění heslo databázové role.

## Volba přístupu a pošty

V bash relaci připravit základní volbu:

```bash
cd /srv/obecni-web-test
compose_files=(-f compose.staging.yaml)
```

Samotný základ nabízí web na `127.0.0.1:3000` pro SSH tunel a zachytává všechnu
poštu do Mailpitu. Pro veřejný web přes HTTPS přidat:

```bash
compose_files+=(-f compose.staging-https.yaml)
```

HTTPS varianta odstraní publikování portu 3000 a zpřístupní pouze Caddy na
TCP 80 a 443. DNS musí směřovat na VPS a firewall tyto porty povolit.
Frontend je veřejný bez dalšího hesla. Administrace `/admin` používá vlastní
aplikační přihlášení. HTTPS proxy posílá HSTS s platností jeden den pro danou
doménu, také v neprodukčním režimu aplikace. Hlavička `X-Robots-Tag: noindex, nofollow` odrazuje
vyhledávače od indexace, neomezuje veřejný přístup.

Výchozí Mailpit neodesílá ven a nemá vnější síťovou cestu. Jeho rozhraní je
dostupné přes SSH přímo na interní IP kontejneru, postup je níže. Aplikace
v režimu zachytávání ignoruje uložené nastavení Google. SMTP ani HTTP port
Mailpitu se na hostitele nepublikují.

Pouze po výslovném požadavku provozovatele na skutečné odesílání nastavit
`SMTP_HOST`, `SMTP_PORT`, `SMTP_TLS`, `SMTP_USERNAME` a `EMAIL_FROM`, připravit
`smtp-password.txt` a přidat:

```bash
compose_files+=(-f compose.staging-smtp.yaml)
```

Tato varianta vypne zachytávání v aplikaci. Uložené nastavení Google z
administrace pak může přepsat SMTP prostředí, účinné nastavení proto ověřit
v administraci. Pracovat s novou testovací databází, nekopírovat reálné odběratele.
Pro návrat k Mailpitu vynechat SMTP overlay a znovu vytvořit webový kontejner.

Pro zvolenou kombinaci používat ve stejné bash relaci:

```bash
dc() {
    sudo docker compose --env-file deploy/.env "${compose_files[@]}" "$@"
}
dc config -q
dc up -d --wait
dc ps
```

Kontrola `config -q` nevypisuje interpolovanou konfiguraci. Při každé aktualizaci
použít tutéž kombinaci souborů, jinak se změní režim pošty nebo přístupu.

## První ověření

```bash
dc exec web ./obec-admin admin@example.test
dc exec monitor python3 scripts/monitor.py check --database-disk /app/database-volume
dc logs --tail=50 web monitor
```

Adresu administrátora nahradit skutečnou. Nástroj se na heslo zeptá interaktivně,
výchozí minimum je 24 znaků. Ověřit přihlášení, zápis stránky a přílohu.
V HTTPS režimu ověřit certifikát, přesměrování z HTTP a veřejný frontend.
Nepřihlášený požadavek na `/api/v1/admin/session` musí skončit `401`.
Přihlašovací cookie musí mít `Secure`, `HttpOnly` a `SameSite=Strict`.

V základním režimu otevřít z počítače správce tunel:

```bash
ssh -N -o ExitOnForwardFailure=yes \
  -L 127.0.0.1:3000:127.0.0.1:3000 obecni-test
```

Web je potom na `http://127.0.0.1:3000`. Alias SSH přizpůsobit skutečně
předanému přístupu.

Pro Mailpit nejprve na VPS zjistit aktuální interní IP:

```bash
mailpit_container=$(dc ps -q mailpit)
sudo docker inspect --format '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$mailpit_container"
```

Z počítače správce otevřít samostatný tunel a `MAILPIT_IP` nahradit tímto výstupem:

```bash
ssh -N -o ExitOnForwardFailure=yes \
  -L 127.0.0.1:8025:MAILPIT_IP:8025 obecni-test
```

V prohlížeči použít `http://127.0.0.1:8025`. Po znovuvytvoření kontejneru IP
znovu zjistit. Tento přístup funguje i v HTTPS režimu aplikace a zachovává
Mailpit v síti bez vnějšího připojení. Na publikování portu kontejneru
připojeného pouze k `internal` síti nelze spoléhat. Docker ale výslovně
umožňuje [přímý přístup hostitele k interním IP kontejnerů](https://docs.docker.com/reference/cli/docker/network/create/#network-internal-mode---internal).

## Import a hodinový rozvrh

Upravit cestu pracovní složky a Dockeru v jednotce, pokud se liší. Instalace
jednotek sama nezapíná import ani časovač:

```bash
sudo install -o root -g root -m 0644 deploy/systemd/obecni-web-legacy-sync.service /etc/systemd/system/
sudo install -o root -g root -m 0644 deploy/systemd/obecni-web-legacy-sync.timer /etc/systemd/system/
sudo systemd-analyze verify /etc/systemd/system/obecni-web-legacy-sync.service /etc/systemd/system/obecni-web-legacy-sync.timer
sudo systemctl daemon-reload
```

První import spustit ihned na pokyn provozovatele:

```bash
sudo systemctl start obecni-web-legacy-sync.service
sudo journalctl -u obecni-web-legacy-sync.service -n 80 --no-pager
dc exec web cat /app/migration/last-run.json /app/migration/last-success.json
```

Ověřit `status: ok`, aktuální čas dokončení, počty a ukázkové přílohy ve webu.
Stav `skipped` nedokládá úspěšné dokončení. Odpovědi HTTP 404 import přeskakuje
a zaznamenává do `/app/migration/capture-unavailable.json`, ostatní chyby
stahování zůstávají důvodem neúspěchu. Nové dokumenty a události se publikují.
Přepínač `--skip-pages` vynechá obsahové stránky a galerie. Přepínač
`--archive-notices` ukládá položky původní
úřední desky rovnou do veřejného archivu a zachovává jejich názvy a přílohy.
Nevyplněná zdrojová data zůstávají neznámá, datum importu se nevydává za
datum vyvěšení nebo sejmutí. Import neodesílá oznámení odběratelům.
Již importované koncepty vyžadují [jednorázový převod](migration.md#archive-existing-imported-notices).
Až po úspěšném prvním importu zapnout rozvrh:

```bash
sudo systemctl enable --now obecni-web-legacy-sync.timer
systemctl list-timers obecni-web-legacy-sync.timer
```

Spouští se 16krát denně, v 07:00 až 22:00 včetně v časové zóně Europe/Prague.
Zmeškané běhy se nedohánějí a souběžné importy se nespouštějí. Jednotka používá
základní Compose pouze k `exec` do běžícího webu, takže zachová skutečné HTTPS
i SMTP prostředí kontejneru. Před změnou domény původního webu časovač vypnout
a ověřit dokončení běžící služby.

## Provoz na 2 GB

Limity webu, PostgreSQL, monitoru, Mailpitu a Caddy mají dohromady 1440 MiB.
Import sdílí 768 MiB webového kontejneru. Při prvním importu a práci s obrázky
zkontrolovat `sudo docker stats --no-stream`, `free -h`, volné místo a OOM
události. Limity jsou výchozí nastavení, ne potvrzená kapacitní záruka.

Databáze, importní stav a další provozní data používají trvalé svazky.
Zálohování není zapnuté a monitor výslovně vynechává kontrolu záloh. Nadále
kontroluje připravenost webu, databázi, místo na disku a poštovní frontu.
`docker compose down -v` by smazalo trvalá data, pro běžnou aktualizaci ho nepoužívat.
