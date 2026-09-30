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
if(!process.exitCode)console.log('polkit rules OK');
