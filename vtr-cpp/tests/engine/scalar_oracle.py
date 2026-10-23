"""Decode byte-plane pages independently of the C++ reader and type traversal."""
import subprocess
import sys
from schema_oracle import Cursor
from records_oracle import decompress
for codec in range(3):
    block=bytes.fromhex(subprocess.check_output([sys.argv[1],str(codec)],text=True))
    r=Cursor(block)
    assert r.fixed(8)==7
    bounds=[r.fixed(8) for _ in range(14)]
    assert bounds[:4]==[2000,0,1,3999]
    generators,pages=r.fixed(4),r.fixed(4)
    ids=[r.fixed(4) for _ in range(generators)]
    assert ids==[4,9]
    headers=[([r.fixed(8) for _ in range(14)],[r.fixed(4) for _ in range(3)]) for _ in range(pages)]
    blobs=r.take(len(block)-r.offset)
    observed=[]
    for b,(offset,length,raw_length) in headers:
        raw=decompress(blobs[offset:offset+length]); assert len(raw)==raw_length<=4096
        page=Cursor(raw);assert page.fixed(1)==2 and page.fixed(4)==23
        n=page.fixed(4);assert n==b[0] and b[1]==0
        columns=[]
        for _ in range(9):
            width=page.fixed(1);assert width<=8
            planes=page.take(width*n)
            values=[sum(planes[byte*n+i]<<(8*byte) for byte in range(width)) for i in range(n)]
            assert width==max(values).bit_length()//8+(max(values).bit_length()%8!=0)
            columns.append(values)
        page.done()
        for row in zip(*columns):
            record,gen,begin,duration,parent,classification,*args=row
            i=(record-1)//2
            assert record==i*2+1 and ids[gen]==(4 if i%2 else 9)
            assert begin==(i*117)%9000 and duration==i%99 and parent==(1 if i else 0)
            assert classification==(i%6)*16+i%5
            assert args==[i,i%19,0 if i%3 else 2**64-1]
            observed.append(i)
    assert observed==list(range(2000))
print('Independent byte-plane oracle passed for stored, LZ4 and Zstandard pages')
