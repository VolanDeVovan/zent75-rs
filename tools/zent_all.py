import os, select, time
fd=os.open('/dev/hidraw7',os.O_RDWR)
hb=b'\x00'+bytes([0xBE,0,5,0,0xED]).ljust(1024,b'\x00')
N=83
names="Esc F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12 Print Home ` 1 2 3 4 5 6 7 8 9 0 - = Bksp Del Tab Q W E R T Y U I O P [ ] \\ PgUp Caps A S D F G H J K L ; ' Enter PgDn LShift Z X C V B N M , . / RShift Up End LCtrl LWin LAlt Space RAlt FN RCtrl Left Down Right".split()
mx=[0]*N; lo=[10**6]*N; hi=[0]*N
end=time.time()+30; last=0
while time.time()<end:
    if time.time()-last>1: os.write(fd,hb); last=time.time()
    r,_,_=select.select([fd],[],[],0.1)
    if not r: continue
    t=os.read(fd,1025)
    if t[0]!=0xBE or t[1]!=0xFD or t[2]+256*t[3]!=1017: continue
    v=[t[2*i]+256*t[2*i+1] for i in range(2,2*N+2)]
    for k in range(N):
        mx[k]=max(mx[k],v[N+k]); lo[k]=min(lo[k],v[k]); hi[k]=max(hi[k],v[k])
print("pressed keys (adc swing > 3000):")
for k in range(N):
    if hi[k]-lo[k]>3000:
        flag = "  <-- BROKEN: sensor moved, firmware travel ~0" if mx[k]<500 else ""
        print(f"{names[k]:7} adc {hi[k]}->{lo[k]}  travel {mx[k]/1000:.2f} mm{flag}")
