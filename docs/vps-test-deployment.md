# Nasazení testovacího webu na VPS s Ubuntu

Plán připravený 4. října 2026 pro VPS s Ubuntu Server 26.04 LTS u libovolného
poskytovatele splňujícího níže uvedené požadavky. Cílem je nejprve zabezpečit
nové VPS a předat správci ověřené přihlášení z linuxového počítače. Potom
nasadit testovací web. Dokument je univerzální postup pro nový server.
Připravené Compose soubory, veřejné HTTPS, volbu pošty a běžný provoz
popisuje [provoz testovacího nasazení](staging-deployment.md).

## Dohodnutá konfigurace

| Položka | Nastavení |
| --- | --- |
| VPS | Plná virtualizace, přístup přes root nebo `sudo`, podpora Dockeru |
| Systém | Ubuntu Server 26.04 LTS, čistá instalace |
| Prostředky | 2 vCPU, 2 GB RAM, 40 GB SSD |
| Swap | 2 GB, pouze jako rezerva při krátkých špičkách |
| SSH | Osobní účet správce, přihlášení pouze veřejným klíčem |
| Sestavení aplikace | Na vývojovém počítači, přenos hotového Docker obrazu |
| Databáze | PostgreSQL 18 v interní síti Dockeru |
| Zálohování | Pro tento test se nezřizuje |
| První přístup k webu | Přes SSH tunel, bez veřejného aplikačního portu |
| První import | Ihned na výslovný pokyn, po ověření připraveného testovacího webu |
| Další synchronizace | Denně každou celou hodinu od 07:00 do 22:00 včetně, Europe/Prague |

2 GB RAM jsou výchozí odhad pro několik testerů. Po importu a při práci
s obrázky změříme skutečnou spotřebu a případné zásahy OOM killeru.
Testovací databáze bude obnovitelná novým importem, testovací úpravy mohou
při ztrátě serveru zmizet.

## Požadavky na VPS a vstupní údaje

VPS musí umožňovat Docker, cgroups, vlastní firewall, trvalé diskové úložiště
a swap. U služby založené na kontejnerech nejprve ověřit omezení poskytovatele.
Potřebná je také cesta k opravě přístupu nezávislá na SSH, například webová
nebo sériová konzole či záchranný režim.

- IP adresa dostupná z počítače správce, přidělené IPv4 a IPv6 a skutečný SSH port.
- Počáteční SSH účet a dostupný způsob přihlášení podle dodaného obrazu systému.
- Veřejný SSH klíč nebo cesta k již připravenému klíči na počítači správce.
- Funkční konzole nebo záchranný režim pro případ opravy SSH nebo firewallu.
- Případný firewall poskytovatele a pravidla platná pro veřejnou síť VPS.
- Později doména a rozhodnutí o veřejném nebo omezeném přístupu přes HTTPS.

Postup připouští i VPS pouze s IPv6, pokud k němu má správce a později všichni
testeři konektivitu. V takovém případě navíc ověřit odchozí přístup hostitele
i kontejnerů k repozitářům, registrům obrazů a původnímu webu. Podle potřeby
zajistit IPv6 síť kontejnerů nebo překlad adres. Konkrétní počáteční účet
se nepředpokládá.

Před použitím příkazů a konfigurace nahradit zástupné hodnoty:

| Hodnota | Význam |
| --- | --- |
| `IP_VPS` | Skutečná adresa serveru, případně jeho ověřené DNS jméno |
| `ADMIN_USER` | Zvolené jméno osobního účtu správce |
| `SSH_PORT` | Skutečný SSH port, obvykle 22 |
| `IP_KLIENTA` a `KLIENT` | Adresa a jméno klienta pro vyhodnocení SSH pravidel |

Alias `obecni-test`, cesta `~/.ssh/obecni_test_ed25519` a adresář
`/srv/obecni-web-test` jsou příklady, které lze přizpůsobit konkrétnímu nasazení.

Pro předání přístupu stačí IP, port, uživatelské jméno a způsob autentizace.
Soukromý klíč zůstane na počítači. Pokud první přihlášení vyžaduje heslo,
zadá správce heslo interaktivně do terminálu. Nevložíme ho do příkazu, Git ani
provozního protokolu. Přístup k celému zákaznickému účtu zůstává u vlastníka VPS.

## 1 Ověření serveru a nouzového přístupu

1. V zákaznické administraci ověřit objednaný server, IP adresy a použitelnou
   konzoli nebo záchranný režim. Ověřit funkční přihlášení do OS nebo postup
   zpřístupnění jeho disku v záchranném režimu. Samotné tlačítko konzole nestačí.
2. Nezávislým důvěryhodným kanálem získat otisk hostitelského klíče,
   například přes konzoli pomocí
   `ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`, a porovnat ho při prvním
   připojení z Linuxu. Samotný výstup `ssh-keyscan` neověřuje identitu serveru.
   V záchranném režimu číst klíč instalovaného systému z připojeného disku,
   nikoli klíč samotného záchranného prostředí.
3. Ověřit OS, architekturu, RAM, disk, síť a naslouchající služby. Zkontrolovat
   existující účty, SSH konfiguraci, cloud-init, oba případné firewally,
   stav AppArmor a dostupnost funkcí potřebných pro Docker a swap.
4. Nechat otevřené počáteční administrátorské spojení. Změny SSH a firewallu
   provádět odděleně, s možností časově omezeného návratu původního nastavení.

Podmínkou pokračování je ověřená identita stroje a použitelná cesta k opravě
přístupu nezávislá na SSH. Nový hostitelský klíč po nečekané změně automaticky nepřijímat.

## 2 Osobní klíč a administrátorský účet

1. Na počítači správce vybrat existující vhodný klíč, nebo vytvořit samostatný
   Ed25519 klíč pro tento VPS. Existující soubory se nepřepisují.
2. Soukromý klíč chránit heslovou frází zadanou správcem a zpřístupnit přes
   místního SSH agenta. Na server přenést pouze soubor `.pub`.
3. Na VPS vytvořit zvolený osobní účet `ADMIN_USER`, adresář `.ssh` s právy `0700` a
   `authorized_keys` s právy `0600`, vše ve vlastnictví tohoto účtu.
   Pokud vhodný účet již existuje, ověřit jeho stav a použít jej bez přepisování klíčů.
4. Povolit správu přes `sudo`. Samostatné heslo účtu pro `sudo` a konzoli
   nastaví správce interaktivně. Nezavádět obecné `NOPASSWD: ALL`.
5. V nové nezávislé relaci ověřit přihlášení správným klíčem, identitu účtu
   a funkční `sudo`. Při ověřování nepoužít sdílené existující SSH spojení.
   Použít například volby `-o ControlMaster=no -o ControlPath=none`.

Příklad vytvoření klíče, až po ověření, že uvedená cesta není obsazená:

```bash
ssh-keygen -t ed25519 -a 100 -f ~/.ssh/obecni_test_ed25519 -C obecni-test
ssh-add ~/.ssh/obecni_test_ed25519
```

Heslová fráze odemyká místní klíč. Heslo pro `sudo` umožňuje zvýšení oprávnění
na serveru. Ani jedno neznamená povolení přihlášení heslem přes SSH.

## 3 Přechod SSH pouze na klíče

Nejprve projít `/etc/ssh/sshd_config`, všechny vložené soubory a případné bloky
`Match`. OpenSSH běžně použije první nalezenou hodnotu. Název souboru
`99-hardening.conf` proto sám nezaručuje přepsání nastavení od poskytovatele.
Použít vhodně zařazený vlastní soubor a ověřit výslednou konfiguraci.

Cílové zásady pro jediný administrátorský účet:

```text
PubkeyAuthentication yes
AuthenticationMethods publickey
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
PermitEmptyPasswords no
AllowUsers ADMIN_USER
UsePAM yes
X11Forwarding no
AllowAgentForwarding no
AllowTcpForwarding local
GatewayPorts no
PermitTunnel no
```

Lokální SSH tunel zůstává povolený pro testovací web. Šifry a výměnu klíčů
ponechat na aktuálních bezpečných výchozích hodnotách Ubuntu.

Postup aktivace:

1. Až po úspěchu nového přihlášení klíčem připravit výše uvedené nastavení.
2. Ověřit syntaxi pomocí `sudo sshd -t` a účinné hodnoty pomocí
   `sudo sshd -T -C user=ADMIN_USER,host=KLIENT,addr=IP_KLIENTA`.
   Prověřit také případné výjimky pro root a další zdrojové adresy.
3. Zkontrolovat, zda službu řídí `ssh.service`, `ssh.socket` nebo obě jednotky.
   Pro samotnou autentizaci použít odpovídající reload služby. Port zůstává 22,
   pokud poskytovatel nedodal jiné skutečné nastavení.
4. Ověřit novým spojením klíč, `sudo` a tunel. Ověřit odmítnutí root loginu,
   přihlášení bez platného klíče i metod password a keyboard-interactive.
5. Prověřit cloud-init a další správu konfigurace, aby další start nevrátil
   heslové přihlášení. Přesný zásah přizpůsobit skutečnému obrazu VPS.
6. Teprve po těchto kontrolách zrušit časovaný návrat nastavení a ukončit
   počáteční administrátorské spojení. Nezavést jiný trvalý přístup pro agenta.

Postup vychází z [dokumentace OpenSSH na Ubuntu](https://ubuntu.com/server/docs/how-to/security/openssh-server/)
a [referenční konfigurace OpenSSH](https://man.openbsd.org/sshd_config).

## 4 Firewall a údržba systému

- Před aktivací firewallu povolit skutečný SSH port a ověřit nové spojení.
  Výchozí politika bude zakázaný příchozí provoz a povolený odchozí provoz.
- Během změn držet otevřenou relaci správce s funkčním `sudo`. Ihned po
  aktivaci firewallu a po každém omezení zdrojových IP ověřit nové nezávislé
  SSH spojení a `sudo`. Teprve potom zrušit časovaný návrat pravidel a restartovat.
- V první fázi veřejně zpřístupnit pouze SSH. Má-li správce stabilní veřejnou
  IP, lze ho omezit na tuto adresu. Jinak ponechat klíčové přihlašování
  s omezením četnosti pokusů, aby změna domácí IP nezpůsobila ztrátu přístupu.
- Pravidla uplatnit pro IPv4 i IPv6. Zachovat potřebný ICMP a ICMPv6, zejména
  pro zjišťování MTU a funkci IPv6. Síťovou konfiguraci nepřepisovat naslepo.
- Pokud poskytovatel používá další síťový firewall, povolit potřebný provoz
  i v něm. Jeho nastavení ověřovat samostatně, pravidla uvnitř VPS ho nenahrazují.
- Nainstalovat aktualizace a zapnout automatické bezpečnostní aktualizace
  Ubuntu. Ověřit běh časovačů a zkušební průchod unattended-upgrades.
  Automatické restarty bez oznámení vypnout, nutné restarty provést při správě.
- Udržet aktivní AppArmor, synchronizaci času a omezenou velikost systémových
  i Docker logů. Prověřit a vypnout nepotřebné veřejné služby.
- Zřídit 2 GB swapu s právy `0600`, záznamem ve `fstab` a ověřením po restartu.
  Nevytvářet ho znovu, pokud už vhodný swap existuje.
- Aktualizace Dockeru a nových aplikačních obrazů provádět řízeně. Samotné
  unattended-upgrades neaktualizuje obsah běžících kontejnerů.

Po dokončení provést řízený restart a znovu ověřit nové přihlášení, `sudo`,
firewall, swap a aktivní služby. Tím končí první samostatná etapa a správce
dostane funkční návod k ručnímu přihlášení ještě před nasazením webu.

Viz [firewall Ubuntu](https://ubuntu.com/server/docs/how-to/security/firewalls/)
a [automatické aktualizace](https://ubuntu.com/server/docs/how-to/software/automatic-updates/).

## 5 Příprava testovací konfigurace aplikace

Současný projekt má produkční Compose, nikoli hotovou konfiguraci pro tento
úsporný test. Před prvním startem připravit samostatný `compose.staging.yaml`
a příslušné příklady konfigurace. Produkční chování zachovat.

Konkrétní práce v repozitáři:

1. Použít již sestavený obraz místo `build:`. Označit ho revizí zdrojů
   a ověřit, že odpovídá architektuře VPS. Stávající CI obraz testuje,
   ale zatím ho nepublikuje. Pro první nasazení použít `docker save`, přenos
   přes SSH a `docker load`, bez potřeby zřizovat registr.
2. Test spouštět s `OBEC_PRODUCTION=false` a `OBEC_UKAZKOVA_DATA=false`
   v release sestavení, nejprve s přístupem přes tunel. Pravý produkční režim
   vyžaduje schválené údaje o soukromí a obsahové stránky. Kvůli testu tyto
   souhlasy nevyrábět.
3. Do testovacího Compose vůbec nezařadit službu `backup`. Monitor doplnit
   o explicitní vypnutí kontrol záloh, při zachování kontrol aplikace,
   databáze, volného místa a pošty. Výchozí produkční kontrolu záloh zachovat.
4. PostgreSQL začít přibližně s `shared_buffers=128MB`, `work_mem=4MB`,
   `maintenance_work_mem=64MB` a `max_connections=30`. Aplikace nyní používá
   nejvýše deset spojení. Jde o výchozí ladění, spotřebu ověřit měřením.
5. Zachovat omezení oprávnění webového kontejneru, read-only souborový systém,
   trvalé databázové úložiště a limity logů. Paměťové limity všech služeb
   stanovit s rezervou pro OS a ověřit při importu i nahrávání obrázků.
6. Ve výchozí variantě použít interní Mailpit bez předávání zpráv ven.
   Jeho webové rozhraní zpřístupnit přes SSH tunel na interní IP kontejneru.
   Začít novou databází a testovacími účty, bez reálných odběratelů a Google
   SMTP přihlašovacích údajů. Uložené nastavení Google dnes přepisuje SMTP
   proměnné prostředí, samotná změna `SMTP_HOST` proto nestačí při kopii DB.
7. V režimu zachytávání nastavit `OBEC_SMTP_CAPTURE_ONLY=true`. Tím se
   ignorují uložené Google údaje a blokuje změna účtu přes administraci.
   Odeslání skutečného e-mailu nesmí být součástí ověřovacího testu.
   Pokud provozovatel výslovně požaduje skutečné SMTP, použít samostatný
   [SMTP overlay](staging-deployment.md#volba-přístupu-a-pošty), chráněný soubor
   hesla a ověřit účinné nastavení. Autentizaci lze ověřit bez odeslání zprávy.

Před použitím ověřit Compose, start služeb, stav monitoru bez záloh a zachycení
e-mailu. U změn monitoru a směrování pošty doplnit cílené regresní testy.

Relevantní soubory:

- [Produkční Compose](../compose.production.yaml)
- [Docker obraz webu](../Dockerfile.web)
- [Stávající provozní postup](operations.md)
- [Monitor](../scripts/monitor.py)
- [SMTP nastavení](../src/backend/mail_settings.rs)
- [Produkční vstupní kontroly](../src/config.rs)

## 6 Instalace Dockeru a první start

1. Instalovat Docker Engine a Compose plugin z oficiálního podepsaného
   repozitáře pro Ubuntu 26.04. Nepoužívat instalační skript stažený rovnou
   do shellu. Dokumentace Dockeru tuto verzi Ubuntu aktuálně podporuje.
2. Správu Dockeru provádět přes `sudo`. Členství ve skupině `docker` by účtu
   poskytlo prakticky oprávnění roota. Nezpřístupňovat Docker socket ani API.
3. Založit samostatný projekt například v `/srv/obecni-web-test`. Přenést
   pouze obraz a potřebné konfigurační soubory, bez vývojového `target/`,
   kompletního `data/`, soukromých klíčů nebo vývojových tajných údajů.
4. Na VPS vytvořit nová náhodná hesla PostgreSQL a aplikační databázové role.
   Tajné soubory držet mimo Git, s omezeným přístupem a čitelností jen pro
   potřebné účty kontejnerů. Výpis Compose ani prostředí se secrety nelogovat.
5. Spustit PostgreSQL, aplikovat migrace aktuální aplikací a vytvořit nové
   testovací administrátory. Připravit také privátní zapisovatelný svazek
   `migration-state` připojený do `/app/migration`. První import a následný
   rozvrh spustit podle následující etapy po ověření funkčnosti webu.
6. Zveřejnit web pouze jako `127.0.0.1:3000:3000`. PostgreSQL ani porty Mailpitu
   nepublikovat. Mailpit ponechat v interní síti bez vnějšího připojení a jeho
   UI otevřít přes SSH přímo na interní IP kontejneru podle
   [provozního postupu](staging-deployment.md#první-ověření).
7. Vyzkoušet web přes tunel, přihlášení, uložení stránky, nahrání obrázku,
   soubor ke stažení a zachycení testovací pošty. Změřit RAM, swap a disk.

Publikované Docker porty mohou obejít pravidla UFW. Ochrana proto stojí také
na kontrole každého `ports:` a skutečných pravidel přeposílání paketů.
Při dalších pravidlech respektovat aktivní Docker firewall backend.
Po instalaci prověřit dostupnost portů z jiného stroje přes všechny přidělené
veřejné IP verze. Ověřit také, že nezůstala nečekaná veřejná IPv6.

Viz [instalace Dockeru](https://docs.docker.com/engine/install/ubuntu/),
[Docker a firewall](https://docs.docker.com/engine/network/packet-filtering-firewalls/)
a [oprávnění skupiny docker](https://docs.docker.com/engine/install/linux-postinstall/).

## 7 První import a hodinová synchronizace

Po prvním funkčním spuštění nabídnout provozovateli okamžitý import obsahu
z původního webu. Po potvrzení ho spustit hned, i mimo pravidelné časové okno.
Pokud už provozovatel výslovně požádal o spuštění, znovu se neptat.
Dokud není první import vyžádán a úspěšně ověřen, časovač zůstává vypnutý.

Použít stávající `scripts/legacy_sync.py run` pro první naplnění i další běhy.
Na prázdné databázi vytvoří nové záznamy. Při opakování rozpozná již převzaté
položky a nezdvojuje je. Každý běh znovu stáhne zdrojový web, přidá nové položky
a změny existujících položek uloží ke kontrole. Lokální úpravy nepřepisuje
a zmizelé zdrojové položky automaticky nemaže.

Pro obsah tohoto projektu použít mapu `config/legacy-vysker-notices.json`.
Mapa souvisí se zdrojovým webem, na poskytovateli VPS nezávisí.
Přepínače `--publish-content --skip-pages` zpřístupní nové dokumenty a události
a vynechají obsahové stránky i galerie. Ve veřejném HTTPS režimu jsou dostupné všem.
Přepínač `--archive-notices` uloží položky původní úřední desky do veřejného
archivu s jejich názvy a přílohami. Předchozí koncepty se automaticky nemění a import
neodesílá oznámení odběratelům. Podrobnosti jsou v [postupu migrace](migration.md).

První i plánované spuštění vést přes stejnou službu systemd. Budoucí soubor
`/etc/systemd/system/obecni-web-legacy-sync.service`:

```ini
[Unit]
Description=Import obsahu puvodniho webu do testovaciho prostredi
Wants=network-online.target
Requires=docker.service
After=network-online.target docker.service

[Service]
Type=oneshot
User=root
WorkingDirectory=/srv/obecni-web-test
ExecStart=/usr/bin/docker compose --env-file deploy/.env -f compose.staging.yaml exec -T web python3 scripts/legacy_sync.py run --state /app/migration --notice-map config/legacy-vysker-notices.json --publish-content --archive-notices --skip-pages
TimeoutStartSec=infinity
```

Cestu k Dockeru ověřit pomocí `command -v docker`. Pracovní adresář, env soubor,
název projektu a Compose musí odpovídat skutečně spuštěnému testovacímu webu.
Soubory služby a časovače bude vlastnit root. Služba spravuje Docker s oprávněním
roota, Python uvnitř webového kontejneru běží pod jeho neprivilegovaným účtem.
Import sdílí paměťový limit webu, proto při prvním běhu změřit spotřebu.

Po připravení jednotek a potvrzení prvního importu:

```bash
sudo systemd-analyze verify /etc/systemd/system/obecni-web-legacy-sync.service /etc/systemd/system/obecni-web-legacy-sync.timer
sudo systemctl daemon-reload
sudo systemctl start obecni-web-legacy-sync.service
sudo journalctl -u obecni-web-legacy-sync.service -n 80 --no-pager
```

Zkontrolovat návratový stav a `/app/migration/last-run.json`,
`last-success.json` a `capture-errors.json`. Ověřit počty položek, ukázkové
přílohy a fotografie v testovacím webu. Stav `skipped` není důkaz dokončeného
prvního importu, při souběhu počkat na skutečný úspěšný běh.

Po úspěchu aktivovat hodinový rozvrh. Budoucí soubor
`/etc/systemd/system/obecni-web-legacy-sync.timer`:

```ini
[Unit]
Description=Hodinova synchronizace obsahu od 07 do 22 hodin

[Timer]
OnCalendar=*-*-* 07..22:00:00 Europe/Prague
AccuracySec=1s
RandomizedDelaySec=0
Persistent=false
Unit=obecni-web-legacy-sync.service

[Install]
WantedBy=timers.target
```

Rozvrh znamená **16 plánovaných startů denně: 07:00, 08:00, …, 22:00**.
Časová zóna je uvedená přímo v časovači, takže odpovídá českému letnímu
i zimnímu času i na serveru s hodinami nastavenými na UTC. Přesnost skutečného
startu závisí také na zatížení serveru. Rozvrh omezuje začátky, běh zahájený
ve 22:00 může doběhnout později.

`Persistent=false` zajistí, že se po zapnutí serveru nedohání zmeškané běhy
mimo rozvrh. Nepřidávat `OnBootSec` ani automatický restart neúspěšného importu.
Souběžně neinstalovat původní `deploy/legacy-sync.cron.example`, který plánuje
spouštění po celých 24 hodin.

```bash
systemd-analyze calendar --iterations=18 '*-*-* 07..22:00:00 Europe/Prague'
sudo systemctl enable --now obecni-web-legacy-sync.timer
systemctl list-timers obecni-web-legacy-sync.timer
```

Kontroly a chování při chybě:

- Systemd znovu nespustí stejnou službu, pokud předchozí běh ještě probíhá.
  Skript navíc používá databázový zámek pro celý běh, takže chrání i před
  souběžným přímým spuštěním. Přeskočené hodiny se nehromadí do fronty.
- HTTP 404 se přeskočí a uloží do `capture-unavailable.json`. Ostatní chyby
  stahování ve výchozím stavu ukončí běh chybou před importem.
  `--allow-incomplete` se do automatického příkazu nepřidává.
- Při chybě zapsat neúspěch, zachovat poslední úspěšný stav a další automatický
  pokus provést v nejbližším plánovaném termínu. Ruční opakování je možné na pokyn.
- Kontrolovat poslední úspěšný běh a frontu změn ke kontrole. Monitor musí
  respektovat noční přestávku, nemá hlásit výpadek jen proto, že je například 03:00.
  Běh delší než hodinu označit k prověření, nespouštět další kopii.
- Reporty jsou privátní v `migration-state`, logy ukládá journal s nastaveným
  omezením velikosti. Zálohování se kvůli importu nezapíná.
- Před přepnutím původní domény na novou aplikaci časovač vypnout pomocí
  `sudo systemctl disable --now obecni-web-legacy-sync.timer`. Vypnutí časovače
  nezastaví právě běžící službu, před přepnutím domény ověřit i její dokončení.

Jde o plán jednotek na VPS. Jejich uvedení v dokumentu nic neinstaluje ani
nespouští. Kalendářní výraz byl ověřen pomocí `systemd-analyze calendar`,
celé jednotky je nutné před aktivací ověřit na cílovém VPS. Časovače popisuje
[dokumentace projektu systemd](https://github.com/systemd/systemd/blob/main/man/systemd.timer.xml).

## 8 Testovací doména a HTTPS

Až bude známá doména a zvolený přístup:

1. Nastavit A pro přidělenou IPv4 a AAAA pro ověřenou funkční IPv6 podle
   dostupných adres. U VPS pouze s IPv6 použít AAAA a ověřit dostupnost
   od testerů i certifikační autority. Produkční doménu obce nepřepínat.
2. Připravit samostatný Caddyfile pro test. Pro veřejný frontend použít
   `compose.staging-https.yaml` bez vstupního hesla. Administrace `/admin`
   používá vlastní přihlášení silným heslem. Pokud provozovatel požaduje
   uzavřený test, doplnit omezení přístupu k celému webu nebo zachovat tunel.
3. Zapnout Caddy a veřejné TCP porty 80 a 443 pro vydání certifikátu a HTTPS
   v hostitelském i případném síťovém firewallu poskytovatele.
   HTTP přesměrovat na HTTPS. UDP 443 ponechat pro první test zavřený.
4. Ověřit certifikát, obnovování certifikátu, zvolený přístup, skutečnou
   klientskou IP a odmítnutí nepřihlášených požadavků na administrátorské API.
   Zachovat shodu interní IP proxy a `OBEC_TRUSTED_PROXY`. V aplikaci
   přepnout veřejnou URL na testovací HTTPS doménu a ověřit generované odkazy.
5. Odstranit dočasné publikování webového portu 3000. Interní web zůstane
   za Caddy. Zachovat označení neoficiálního webu a `X-Robots-Tag: noindex, nofollow`.
   Zákaz indexace nenahrazuje omezení přístupu.

## 9 Předání ručního přihlášení

Po ověření zabezpečení doplnit do tohoto postupu skutečný alias, uživatele,
IP, port, cestu k veřejnému klíči a ověřený otisk hostitelského klíče.
Soukromé klíče ani hesla do dokumentace nepatří.

Na linuxovém počítači správce připravit nový záznam v `~/.ssh/config`, bez přepsání
ostatních hostů. Následující hodnoty jsou zatím vzor:

```sshconfig
Host obecni-test
    HostName IP_VPS
    User ADMIN_USER
    Port SSH_PORT
    IdentityFile ~/.ssh/obecni_test_ed25519
    IdentitiesOnly yes
    PreferredAuthentications publickey
    PasswordAuthentication no
    KbdInteractiveAuthentication no
    StrictHostKeyChecking yes
    ForwardAgent no
```

Ověřený hostitelský klíč musí být předem uložený v `known_hosts`.
Potom bude ruční přihlášení:

```bash
ssh obecni-test
```

První testovací web přes tunel, dokud běží loopback port 3000:

```bash
ssh -N -o ExitOnForwardFailure=yes -L 127.0.0.1:3000:127.0.0.1:3000 obecni-test
```

V prohlížeči otevřít `http://127.0.0.1:3000`. Server musí mít pro tuto etapu
stejně nastavenou veřejnou URL aplikace. Prohlížeč používá místní HTTP,
přenos mezi počítačem a VPS chrání SSH. Tunel ukončit pomocí Ctrl+C.

Předat také příkaz pro kontrolu kontejnerů a logů podle finálního Compose,
postup aktualizace a konkrétní cestu ke konzoli nebo záchrannému režimu
poskytovatele. Pokud klíč přestane být dostupný, touto cestou opravit
`authorized_keys`. SSH hesla kvůli tomu plošně nezapínat.

## Podmínky dokončení

- Správce se novým spojením přihlásí klíčem a může použít `sudo`.
- Root, hesla a keyboard-interactive nejsou použitelné pro SSH login.
- Po restartu zůstává funkční přístup, firewall, aktualizace a swap.
- Zvenku jsou dostupné pouze záměrně otevřené porty, ověřeno i pro veřejnou
  směrovanou IPv6, pokud ji VPS má.
- Testovací web a databáze přežijí restart, běží bez služby zálohování.
- První import proběhl na pokyn a jeho výsledek byl ověřen.
- Časovač spouští synchronizaci v 07:00 až 22:00 Europe/Prague včetně,
  bez souběhu a bez nočního dohánění zmeškaných běhů.
- Monitor nehlásí chybějící zálohy a nadále kontroluje dostupnost a disk.
- Pošta odpovídá zvolenému režimu. Skutečné SMTP je zapnuté pouze na výslovný
  požadavek, jinak všechny zprávy zachytává interní Mailpit.
- Při reprezentativním testu nejsou pády kvůli paměti ani trvalé zahlcení swapu.
- Předaný návod obsahuje ověřené připojení a konkrétní stav nasazení.
