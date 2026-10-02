"""FH6 string tables: `Stripped/StringTables/<LANG>.zip` holds one `<Table>.str` per UI area (plaintext, not encrypted).  READ-ONLY.

.str layout (little endian): 0x98-byte header, u32 count @0x94, then count x (u32 key, u32 offset), then NUL-terminated UTF-8
strings (offset relative to the end of the key table).
key = hash of the identifier:  h = 0xFFFFFFFF; for each byte c: h = rotl32(h ^ c, 7).
  'Landmarks.IDS_Area_Discovered_ine'  ->  table 'Landmarks', key = strhash('IDS_Area_Discovered_ine')  ->  'Ine'
Database strings are stored as `_&<u64>`; the key is the LOW 32 bits of that number (search every table for it).

    st = StringTables(media)                # language 'EN'
    st.table('CareerRaceCollection')        # [(key, text), ...]
    st.ids('Landmarks.IDS_Area_Discovered_ine')
"""
import struct, zipfile

from fh6common import ci

M32 = 0xFFFFFFFF


def strhash(s):
    h = M32
    for c in s.encode():
        h ^= c
        h = ((h << 7) | (h >> 25)) & M32
    return h


class StringTables:
    def __init__(self, media, lang='EN'):
        self.z = zipfile.ZipFile(ci(media, 'Stripped/StringTables', f'{lang}.zip'))
        self._tab = {}

    def table(self, name):
        """list of (key, text) of `<name>.str`; KeyError if the table does not exist"""
        if name not in self._tab:
            d = self.z.read(name + '.str')
            n = struct.unpack_from('<I', d, 0x94)[0]
            base = 0x98 + 8 * n
            res = []
            for i in range(n):
                k, o = struct.unpack_from('<II', d, 0x98 + 8 * i)
                res.append((k, d[base + o:d.index(b'\0', base + o)].decode('utf-8', 'replace')))
            self._tab[name] = res
        return self._tab[name]

    def ids(self, full):
        """'Table.IDS_Key' -> English text or None"""
        t, _, k = full.partition('.')
        try:
            tab = self.table(t)
        except KeyError:
            return None
        h = strhash(k)
        for kk, s in tab:
            if kk == h:
                return s
        return None

    def lookup_db(self, v):
        """resolve a database string `_&<u64>` (low 32 bits = key) in any table -> text or None"""
        if not (isinstance(v, str) and v.startswith('_&')):
            return v
        h = int(v[2:]) & M32
        for name in self.z.namelist():
            if name.endswith('.str'):
                for kk, s in self.table(name[:-4]):
                    if kk == h:
                        return s
        return None
