"""Forza binary XML ('BXML') decoder + EntityModel helpers.  OPTIONAL - only needed with `--entity-model PATH`.

Why optional: `Stripped/EntityModel.zip` (event slots, photo-challenge spots, time attacks, ...) is ENCRYPTED in the current
Steam install (zip method 22).  A readable older copy (July build, plain deflate zip of BXML files) only exists in a decrypted
`media.zip` that the Horizon Nav creator shared.  So everything built on this is "creator-dump only, not loadable from a
user install" - see docs/game-data/fh6-game-files.md.

BXML layout: 'BXML', u8 version, u32 string_count, u32 size, then string_count x (u16 len + UTF-8), then (1 byte) the node tree.
node = u8 op (bit1 = has attributes, bit2 = has children), string index (u8 if < 256 strings else u16), [u8 nattr, nattr x (name idx, value idx)],
[u8 nchildren, u8 0, children...].
"""
import struct, zipfile


def bxml_decode(d):
    """-> (tag, [(attr, value)], [children])"""
    assert d[:4] == b'BXML'
    n, _sz = struct.unpack_from('<II', d, 5)
    p = 13; strs = []
    for _ in range(n):
        l = struct.unpack_from('<H', d, p)[0]; p += 2
        strs.append(d[p:p + l].decode('utf-8', 'replace')); p += l
    w = 1 if n < 256 else 2
    pos = [p + 1]

    def idx():
        if w == 1:
            v = d[pos[0]]; pos[0] += 1
        else:
            v = struct.unpack_from('<H', d, pos[0])[0]; pos[0] += 2
        return v

    def node():
        op = d[pos[0]]; pos[0] += 1
        name = strs[idx()]; attrs = []; kids = []
        if op & 2:
            na = d[pos[0]]; pos[0] += 1
            for _ in range(na):
                a = strs[idx()]; v = strs[idx()]; attrs.append((a, v))
        if op & 4:
            nk = d[pos[0]]; pos[0] += 2          # child count, then one 0 byte
            for _ in range(nk):
                kids.append(node())
        return (name, attrs, kids)
    return node()


def bxml_text(n, ind=0):
    name, attrs, kids = n
    a = ''.join(f' {k}="{v}"' for k, v in attrs)
    sp = ' ' * ind
    if kids:
        return f'{sp}<{name}{a}>\n' + ''.join(bxml_text(k, ind + 1) for k in kids) + f'{sp}</{name}>\n'
    return f'{sp}<{name}{a}/>\n'


class EntityModel:
    def __init__(self, path):
        self.z = zipfile.ZipFile(path)

    def xml(self, path):
        """decoded text of e.g. 'Entities/Brio/campaign_slots.xml'"""
        return bxml_text(bxml_decode(self.z.read(path)))

    def entities(self, path):
        """[(entity_id, template, body_text)] for every <entity> element (nested ones included; body includes sub-entities)"""
        out = []

        def walk(n):
            if n[0] == 'entity':
                at = dict(n[1]); out.append((at.get('id', ''), at.get('template', ''), ''.join(bxml_text(k) for k in n[2])))
            for k in n[2]:
                walk(k)
        walk(bxml_decode(self.z.read(path)))
        return out


def xyz(s):
    return [float(q) for q in s.split(',')]
