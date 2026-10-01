"""Classify captured files by their source breadcrumb, keeping unknown dates unknown."""
from datetime import date, datetime
import ipaddress
import os
import re
from urllib.parse import urlsplit
from zoneinfo import ZoneInfo

from bs4 import BeautifulSoup
from legacy import dates
from postgres import database_url


def require_local_preview():
    origin = urlsplit(os.environ.get('OBEC_VEREJNA_URL', ''))
    database = urlsplit(database_url())
    def local(host):
        if host == 'localhost':
            return True
        try:
            return ipaddress.ip_address(host or '').is_loopback
        except ValueError:
            return False
    if (os.environ.get('OBEC_PRODUCTION', 'false').lower() != 'false'
            or not local(origin.hostname) or not local(database.hostname)):
        raise ValueError('Notice preview publication requires a local, non-production website and database')


def notice_sections(items, capture):
    sections = {}
    for item in items:
        if 'mime' in item:
            continue
        soup = BeautifulSoup(capture.blob(item['capture']['sha256']), 'html.parser')
        breadcrumb = soup.select_one('.cesta')
        labels = [node.get_text(' ', strip=True).replace('\xa0', ' ')
                  for node in breadcrumb.select('a, span')] if breadcrumb else []
        if any(label.casefold() == 'úřední deska' for label in labels):
            sections[item['key']] = labels
    return sections


def source_dates(item):
    evidence = item.get('evidence', '').replace('\xa0', ' ')
    return dates(evidence) if evidence else item.get('dates', {})


def notice_values(item, categories, sections, category_map, preview=False, as_of=None):
    extracted = source_dates(item)
    posted = date.fromisoformat(extracted['published_on']) if extracted.get('published_on') else None
    end = date.fromisoformat(extracted['withdraw_on']) if extracted.get('withdraw_on') else None
    issues = []
    if posted and end and end < posted:
        issues.append('Original withdrawal precedes posting. Withdrawal date left unknown.')
        end = None
    day = as_of or datetime.now(ZoneInfo('Europe/Prague')).date()
    # A local archive grouping is not evidence of withdrawal. Never store a
    # deadline, import timestamp or default duration as an original posting/end date.
    missing_dates = [name for name, value in [('published_on', posted), ('withdraw_on', end)] if value is None]
    status = 'draft'
    if preview:
        if missing_dates or end <= day:
            status = 'archived'
        elif posted <= day:
            status = 'published'
    category = next((category_map[p] for p in item.get('parents', []) if p in category_map), 'Ostatní')
    if re.search(r'vol[eb]', item['title'], re.I):
        category = 'Volby'
    if category not in categories:
        raise ValueError('Unknown notice category: ' + category)
    evidence = item.get('evidence', '')
    description = 'Převedeno z původního webu.\n\n' + evidence
    if posted is None:
        description += '\n\nDatum vyvěšení není uvedeno.'
    if end is None:
        description += '\n\nDatum sejmutí není uvedeno.'
    if preview and missing_dates:
        description += '\n\nMístní náhled řadí záznam do archivu, protože není doloženo datum vyvěšení nebo sejmutí.'
    review = dict(original_reference=item['url'])
    if preview:
        review.update(archive_title=item['title'][:300],
                      archive_basis='Místní náhled migrace, vyžaduje samostatné posouzení před produkcí.',
                      archive_until=date.max.isoformat())
    return dict(published_on=posted, withdraw_on=end, status=status, description=description,
                category_id=categories[category], review=review, issues=issues,
                metadata=dict(source_dates=extracted, evidence=evidence,
                              navigation={p: sections[p] for p in item.get('parents', []) if p in sections},
                              preview_archive_missing_dates=missing_dates if preview else []))
