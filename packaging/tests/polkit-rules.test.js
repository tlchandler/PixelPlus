// Behavioural test for polkit/50-pixelplus.rules: `node packaging/tests/polkit-rules.test.js`
const fs=require('fs');let rule;
const polkit={Result:{YES:'yes',NO:'no',NOT_HANDLED:'nh'},addRule:f=>rule=f};
eval(fs.readFileSync(process.argv[2] || require("path").join(__dirname, "..", "polkit", "50-pixelplus.rules"),'utf8'));
const A=(id,d={})=>({id,lookup:k=>d[k]});
const t=(user,a,exp)=>{const r=rule(a,{user});if(r!==exp){console.error('FAIL',user,a.id,JSON.stringify(a.lookup('unit')),r,exp);process.exitCode=1}};
t('pixelplus',A('org.freedesktop.NetworkManager.settings.modify.system'),'yes');
t('bob',A('org.freedesktop.NetworkManager.settings.modify.system'),'nh');
t('pixelplus',A('org.freedesktop.login1.reboot'),'yes');
t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:'pixelplus-helper@config-txt:difftx:E.service',verb:'start'}),'yes');
t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:'pixelplus-helper@x;rm.service',verb:'start'}),'nh');
t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:'pixelplus-helper@update.service',verb:'stop'}),'nh');
t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:'ssh.service',verb:'start'}),'nh');
t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:'pixelplus-tts.service',verb:'restart'}),'yes');
t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:'pixelplusd.service',verb:'stop'}),'nh');
t('pixelplus',A('org.freedesktop.udisks2.filesystem-mount'),'nh');
// What pixelplusd actually does (crates/pixelplus-daemon/src/services/platform.rs):
t('pixelplus',A('org.freedesktop.login1.power-off'),'yes');
t('pixelplus',A('org.freedesktop.login1.reboot-multiple-sessions'),'yes');
t('pixelplus',A('org.freedesktop.hostname1.set-static-hostname'),'yes');
t('pixelplus',A('org.freedesktop.hostname1.set-hostname'),'yes');
t('pixelplus',A('org.freedesktop.hostname1.set-machine-info'),'nh');
t('pixelplus',A('org.freedesktop.timedate1.set-timezone'),'yes');
t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:'pixelplusd.service',verb:'restart'}),'yes');
for (const u of ['config-txt:difftxlarge:1600','config-txt:bare-pi','update','ssh-on','ssh-off','reapply','wifi-country:US','hosts'])
  t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:`pixelplus-helper@${u}.service`,verb:'start'}),'yes');
// Least privilege (security audit M6): only the NetworkManager actions nmcli needs, no clock
// setting, and nothing at all for the sidecars' own users.
for (const a of ['network-control','wifi.scan','enable-disable-wifi','settings.modify.own'])
  t('pixelplus',A('org.freedesktop.NetworkManager.'+a),'yes');
for (const a of ['wifi.share.open','wifi.share.protected','settings.modify.hostname','sleep-wake','checkpoint-rollback','reload','enable-disable-network'])
  t('pixelplus',A('org.freedesktop.NetworkManager.'+a),'nh');
t('pixelplus',A('org.freedesktop.timedate1.set-time'),'nh');
t('pixelplus',A('org.freedesktop.timedate1.set-ntp'),'nh');
for (const u of ['pixelplus-games','pixelplus-tts']) {
  t(u,A('org.freedesktop.NetworkManager.network-control'),'nh');
  t(u,A('org.freedesktop.login1.reboot'),'nh');
  t(u,A('org.freedesktop.systemd1.manage-units',{unit:'pixelplus-helper@ssh-on.service',verb:'start'}),'nh');
}
// Fleet verbs (F14/F15); versions like 1.3.0~beta1 arrive systemd-escaped (\x7e).
for (const u of ['update-stage:1.2.3','update-commit:1.3.0\\x7ebeta1','update-rollback','update-verify',
                 'update-channel:beta','tailscale-install','tailscale-up','tailscale-serve:on','tailscale-funnel:off',
                 'tailscale-down','cloudflared-install','cloudflared-quick:on','cloudflared-token','cloudflared-stop'])
  t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:`pixelplus-helper@${u}.service`,verb:'start'}),'yes');
// ...but the tunnels' own units are the helper's business, not the daemon's.
t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:'pixelplus-cloudflared.service',verb:'start'}),'nh');
t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:'tailscaled.service',verb:'stop'}),'nh');
t('pixelplus',A('org.freedesktop.systemd1.manage-units',{unit:'pixelplus-helper@update-stage:1.0$(x).service',verb:'start'}),'nh');
if(!process.exitCode)console.log('polkit rules OK');
