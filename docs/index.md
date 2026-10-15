---
layout: doc
outline: deep
---

# 服务器管理操作手册(Anyone)

> [!IMPORTANT]
> 本页从原有 Docusaurus 手册的正文迁入 VitePress，保留原有命令供查阅；这些历史命令未针对当前两台生产服务器重新审核，不能直接当作 EasyManage 的自动化操作。原 Nginx 站点仍保留，内容不会自动同步到本页。

Ubuntu 22 课题组服务器，多名成员共用；少数管理员有 `sudo`，普通用户只能访问自己的目录和授权项目目录。

## 1. 用户组设计

本服务器上有下列组：

| 组名 | 用途 |
| --- | --- |
| `lab-users` | 普通课题组成员，允许 SSH 登录 |
| `lab-admins` | 管理员组，允许 sudo |
| `proj-*` | 合作项目 |

创建组【root only】：

``` bash
sudo groupadd -f lab-users
sudo groupadd -f lab-admins
sudo groupadd -f proj-*
```

------------------------------------------------------------------------

## 2. 创建普通用户【root only】

例如创建 `student1`：

``` bash
sudo adduser student1
sudo usermod -aG lab-users
id student1
```

普通用户不要加入 `lab-admins`。

## 3. 创建管理员用户【root only】

假设管理员用户名是 `admin1`，请按实际修改：

``` bash
sudo adduser admin1
sudo usermod -aG lab-users,lab-admins
id admin1
```

| 参数 | 含义 |
| --- | --- |
| `usermod` | 修改已有用户 |
| `-G` | 设置用户的附加组 supplementary groups |
| `-a` | append，追加，不覆盖原来的附加组 |
| `-aG` | 追加到指定附加组，最常用写法 |

配置 `lab-admins` 的 sudo 权限【root only】：

``` bash
sudo tee /etc/sudoers.d/lab-admins >/dev/null <<'EOF2'
%lab-admins ALL=(ALL:ALL) ALL
EOF2

sudo chmod 440 /etc/sudoers.d/lab-admins
sudo visudo -c
```

如果输出包含 `parsed OK`，说明 sudoers 配置正确。

## 4. 配置 SSH key 登录【All】

SSH key 登录的基本逻辑是：

``` text
本地电脑生成一对密钥：
  私钥：留在本地，绝对不要发给别人
  公钥：放到服务器用户的 ~/.ssh/authorized_keys 里
```

登录时，服务器用公钥验证本地私钥是否匹配。这样可以避免使用密码登录，提高安全性。

------------------------------------------------------------------------

### 4.1 在本地生成 SSH 公钥

#### Windows PowerShell 生成密钥

在 **Windows 本地电脑** 打开 PowerShell，执行：

``` powershell
ssh-keygen -t ed25519 -C "student1@laptop"
```

其中：

``` text
-t ed25519              使用 ed25519 密钥算法，推荐
-C "student1@laptop"   给密钥加一个备注，方便以后识别
```

执行后会出现：

``` text
Enter file in which to save the key (C:\Users\YourName/.ssh/id_ed25519):
```

一般直接回车，使用默认路径：

``` text
C:\Users\YourName\.ssh\id_ed25519
```

然后会提示：

``` text
Enter passphrase (empty for no passphrase):
```

建议设置一个 passphrase，也可以直接回车不设置。

生成后，本地会得到两个文件：

``` text
C:\Users\YourName\.ssh\id_ed25519       私钥，不能泄露
C:\Users\YourName\.ssh\id_ed25519.pub   公钥，可以放到服务器
```

查看公钥内容：

``` powershell
type $env:USERPROFILE\.ssh\id_ed25519.pub
```

输出类似：

``` text
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIxxxxxxxxxxxxxxxx student1@laptop
```

把这一整行复制下来，后面要放到服务器用户自己的 `~/.ssh/authorized_keys` 文件里。

------------------------------------------------------------------------

#### Linux / macOS 生成密钥

在本地终端执行：

``` bash
ssh-keygen -t ed25519 -C "student1@laptop"
```

默认生成位置：

``` text
~/.ssh/id_ed25519
~/.ssh/id_ed25519.pub
```

查看公钥：

``` bash
cat ~/.ssh/id_ed25519.pub
```

复制输出的整行公钥。

------------------------------------------------------------------------

### 4.2 给自己的服务器账号添加公钥

以服务器用户 `student1` 为例。

前提是：服务器当前还允许 `student1` 使用密码登录

先从本地电脑登录服务器：

``` powershell
ssh student1@192.168.10.110
```

登录成功后，在服务器上执行下面的命令。

------------------------------------------------------------------------

#### 方法 A：手工粘贴公钥到 `authorized_keys`

``` bash
mkdir -p ~/.ssh
vim ~/.ssh/authorized_keys
```

把本地生成的公钥粘进去，例如：

``` text
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIxxxxxxxxxxxxxxxx student1@laptop
```

一个用户可以有多个公钥，每个公钥占一行。

保存后修复权限：

``` bash
chmod 700 ~/.ssh
chmod 600 ~/.ssh/authorized_keys
```

确认权限：

``` bash
ls -ld ~/.ssh
ls -l ~/.ssh/authorized_keys
```

期望结果类似：

``` text
drwx------ 2 student1 student1 4096 ... /home/student1/.ssh
-rw------- 1 student1 student1  100 ... /home/student1/.ssh/authorized_keys
```

------------------------------------------------------------------------

#### 方法 B：从 `.pub` 文件追加到 `authorized_keys`

如果你已经把本地公钥文件上传到了服务器，例如：

``` text
/tmp/student1_id_ed25519.pub
```

可以直接追加到 `authorized_keys`：

``` bash
mkdir -p ~/.ssh
touch ~/.ssh/authorized_keys

cat /tmp/student1_id_ed25519.pub >> ~/.ssh/authorized_keys

chmod 700 ~/.ssh
chmod 600 ~/.ssh/authorized_keys
```

注意这里必须使用：

``` bash
>>
```

表示追加写入。

不要误用：

``` bash
>
```

因为 `>` 会覆盖原来的 `authorized_keys`，可能导致你已有的其他 SSH key 失效。

------------------------------------------------------------------------

#### 方法 C（补充）：加固写法，避免重复添加同一个公钥

如果担心重复执行导致同一个公钥被追加多次，可以用下面的写法：

``` bash
PUB_FILE="/tmp/student1_id_ed25519.pub"
AUTH_KEYS="$HOME/.ssh/authorized_keys"

mkdir -p "$HOME/.ssh"
touch "$AUTH_KEYS"

grep -qxF "$(cat "$PUB_FILE")" "$AUTH_KEYS" || cat "$PUB_FILE" >> "$AUTH_KEYS"

chmod 700 "$HOME/.ssh"
chmod 600 "$AUTH_KEYS"
```

这段命令的含义是：

``` text
如果 authorized_keys 里已经有这个公钥，就不重复添加；
如果没有，就追加进去。
```

------------------------------------------------------------------------

#### 方法 D：从本地电脑直接通过 SSH 追加公钥

如果服务器当前允许密码登录，可以不先上传 `.pub` 文件，而是从本地直接把公钥传过去。

##### Windows PowerShell

在 Windows 本地执行：

``` powershell
type $env:USERPROFILE\.ssh\id_ed25519.pub | ssh student1@192.168.10.110 "mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys && chmod 700 ~/.ssh && chmod 600 ~/.ssh/authorized_keys"
```

##### Linux / macOS

在本地终端执行：

``` bash
cat ~/.ssh/id_ed25519.pub | ssh student1@192.168.10.110 "mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys && chmod 700 ~/.ssh && chmod 600 ~/.ssh/authorized_keys"
```

------------------------------------------------------------------------

#### 最后确认

查看 `authorized_keys` 内容：

``` bash
cat ~/.ssh/authorized_keys
```

确认权限：

``` bash
ls -ld ~/.ssh
ls -l ~/.ssh/authorized_keys
```

退出服务器：

``` bash
exit
```

然后在本地测试 SSH key 登录：

``` powershell
ssh student1@192.168.10.110
```

------------------------------------------------------------------------

### 4.3 从本地测试 SSH key 登录

#### Windows PowerShell

``` powershell
ssh student1@192.168.10.110
```

如果使用默认私钥路径 `id_ed25519`，通常不需要额外指定私钥。

如果你生成密钥时用了自定义文件名，例如：

``` text
C:\Users\YourName\.ssh\lab_server_ed25519
```

则使用：

``` powershell
ssh -i $env:USERPROFILE\.ssh\lab_server_ed25519 student1@192.168.10.110
```

------------------------------------------------------------------------

#### Linux / macOS

``` bash
ssh student1@192.168.10.110
```

如果指定私钥：

``` bash
ssh -i ~/.ssh/lab_server_ed25519 student1@192.168.10.110
```

------------------------------------------------------------------------

### 4.4 可选：配置本地 SSH 快捷登录

在 Windows 本地创建或编辑：

``` powershell
notepad $env:USERPROFILE\.ssh\config
```

加入：

``` text
Host lab-server
    HostName 192.168.10.110
    User student1
    Port 22
    IdentityFile ~/.ssh/id_ed25519
```

以后可以直接：

``` powershell
ssh lab-server
```

Linux / macOS 对应文件是：

``` bash
vim ~/.ssh/config
```

内容同样为：

``` text
Host lab-server
    HostName 192.168.10.110
    User student1
    Port 22
    IdentityFile ~/.ssh/id_ed25519
```

------------------------------------------------------------------------

### 4.5 QA

如果登录失败，不要关闭原来的可用终端，先检查：

``` text
1. 公钥是否完整复制到 ~/.ssh/authorized_keys
2. ~/.ssh 权限是否为 700
3. ~/.ssh/authorized_keys 权限是否为 600
4. 服务器是否已经提前禁用了密码登录
5. 本地是否使用了正确的私钥文件
```

## 5. 文件目录设计

目录：

``` text
/home/用户名                  用户个人目录
/data/shared                 课题组共享目录
/data/projects/*             合作项目目录
/data/scratch                临时计算目录
/data/archive                归档目录
```

创建目录【root only】：

``` bash
sudo mkdir -p /data/shared
sudo mkdir -p /data/scratch
sudo mkdir -p /data/archive
```

------------------------------------------------------------------------

## 6. 设置共享目录权限【root only】

### 6.1 课题组共享目录

``` bash
sudo chgrp -R lab-users /data/shared
sudo chmod 2775 /data/shared
```

`2775` 中的 `2` 是 setgid 位，新文件会继承父目录的 group。

### 6.2 临时计算目录

``` bash
sudo chgrp -R lab-users /data/scratch
sudo chmod 3777 /data/scratch
```

`3777` 包含 sticky bit，用户不能随便删除别人的临时文件。

### 6.3 配置 ACL

安装 ACL：

``` bash
sudo apt update
sudo apt install acl -y
```

给 `/data/shared` 设置长期共享权限

目标：

- `/data/shared` 目录本身是 `775`
- 新建文件夹默认继承 `lab-users` 组
- 新建文件夹尽量保持 `775`
- 新建普通文件默认给 `lab-users` 组读写权限
- root 或其他用户写入后，`lab-users` 组成员仍可继续读写

设置 `/data/shared` 的所属组：

``` bash
sudo chown -R root:lab-users /data/shared
```

设置现有目录权限为 `2775`：

``` bash
sudo find /data/shared -type d -exec chmod 2775 {} \;
```

说明：

``` text
2775 里的 2 表示 setgid。
作用是：以后在 /data/shared 下面新建的文件和文件夹，会自动继承 lab-users 这个组。
```

设置现有普通文件权限：

``` bash
sudo find /data/shared -type f -exec chmod 775 {} \;
```

配置 ACL，让以后新写入的内容默认给 `lab-users` 组读写执行权限：

``` bash
sudo setfacl -R -m g:lab-users:rwx /data/shared
sudo setfacl -R -m m::rwx /data/shared

sudo setfacl -R -d -m g:lab-users:rwx /data/shared
sudo setfacl -R -d -m m::rwx /data/shared
```

- **`setfacl`**：设置文件访问控制列表（Set File Access Control Lists）。
- **`-R`** (Recursive)：**递归**。意思是不仅修改 `/data/shared` 这个目录本身，还要把它里面目前已经存在的所有子文件夹和文件统统修改一遍。
- **`-m`** (Modify)：**修改**当前的 ACL 规则。
- **`g:lab-users:rwx`**：将 `lab-users` 这个用户组（**g**roup）的权限设置为可读（**r**）、可写（**w**）、可执行/进入目录（**x**）。
- **`m::rwx`**：设置有效权限掩码（**m**ask）为 `rwx`。
  - *为什么要这一行？* 在 ACL 机制中，mask 就像是一个“权限天花板”。如果你给群组分配了 `rwx` 权限，但 mask 只有 `r-x`，那么群组实际能用的最高权限也就只有 `r-x`（写权限被没收了）。这行命令确保刚才赋予的 `rwx` 权限能真实生效，不被系统限制。
- **`-d`** (Default)：**默认** ACL 规则。这一组命令加上了 `-d` 参数，它是这段脚本的灵魂所在。
  - *作用*：前两行虽然改了现有的文件，但如果明天张三在这个目录里新建了一个文件夹，默认情况下 `lab-users` 组的其他人可能还是没法编辑它。加上 `-d` 后，相当于给 `/data/shared` 定下了一个“继承法则”：以后任何人在这里面创建的任何新文件或新目录，都会自动继承给 `lab-users` 组 `rwx` 的权限。

查看权限：

``` bash
sudo getfacl /data/shared
```

验证：

``` bash
sudo -u test bash -lc 'mkdir -p /data/shared/test_dir && touch /data/shared/test_file'

ls -ld /data/shared /data/shared/test_dir /data/shared/test_file
sudo getfacl /data/shared/test_dir
sudo getfacl /data/shared/test_file
```

注意：

``` text
目录可以长期保持 775 或 2775。
其中 2775 比 775 更适合共享目录，因为它能让新建内容继承 lab-users 组。

但是普通文件默认不一定能强制变成 775。

原因是 Linux 里普通文件通常不会默认带执行权限 x。
大多数程序创建普通文件时，初始权限是 666，然后再被 umask 和 ACL 影响。
所以新建普通文件更常见、更推荐的权限是 664。

推荐结果是：

/data/shared         2775
新建文件夹           2775 或 775
新建普通文件         664

这样 lab-users 组成员可以正常读写共享文件，同时不会把所有普通文件都错误地设置成可执行文件。
```

如果强制要求把已有普通文件也改成 `775`，可以执行：

``` bash
sudo find /data/shared -type f -exec chmod 775 {} \;
```

但这只会影响已有文件。以后新建普通文件是否带执行权限，仍然取决于创建文件的程序本身。

------------------------------------------------------------------------

## 7. 防火墙基础配置【暂未开启】【root only】

安装并查看 UFW：

``` bash
sudo apt install ufw -y
sudo ufw status
```

允许 SSH：

``` bash
sudo ufw allow 22/tcp
```

允许 Cockpit 仅内网访问：

``` bash
sudo ufw allow from 192.168.1.0/24 to any port 9090 proto tcp
```

如果你的二级路由器在 1110 网络中的地址是 `192.168.1.102`，也可以只允许该地址访问 Cockpit：

``` bash
sudo ufw allow from 192.168.1.102 to any port 9090 proto tcp
```

启用 UFW：

``` bash
sudo ufw enable
sudo ufw status verbose
```

> 启用防火墙前，一定确认 SSH 端口 22 已允许，否则可能断开远程连接。

------------------------------------------------------------------------

## 8. 环境使用【All】

推荐：

每个用户在自己的 /home/用户名 下安装 Miniforge/Miniconda或相关编程环境

用户自己安装 Miniforge：

目前可用的已下载的在：`/data/shared/software_install_packages/conda`

## 8.1 安装 Conda 环境指南

服务器共享目录中已经准备好了 Conda 安装包和环境配置文件：

``` bash
/data/shared/software_install_packages/conda/miniconda.sh
/data/shared/software_install_packages/conda/cudadev_full_cu126.yml
```

其中 `cudadev_full_cu126.yml` 已经测试可用，推荐直接用它创建 `cudadev` 环境。

### 1. 安装 Miniconda

普通用户不要使用 `sudo` 安装 Conda，建议安装到自己的 home 目录下：

``` bash
bash /data/shared/software_install_packages/conda/miniconda.sh
```

安装过程中，安装路径建议使用默认路径：

``` text
/home/你的用户名/miniconda3
```

安装结束后，重新加载 shell 配置：

``` bash
source ~/.bashrc
```

如果你使用的是 zsh，则执行：

``` bash
source ~/.zshrc
```

确认 Conda 是否安装成功：

``` bash
conda --version
which conda
```

### 2. 创建 cudadev 环境

使用已经测试过的环境文件创建环境：

``` bash
conda env create -f /data/shared/software_install_packages/conda/cudadev_full_cu126.yml
```

创建完成后，激活环境：

``` bash
conda activate cudadev
```

检查 Python 和关键包是否可用：

``` bash
python --version
python -c "import numpy, scipy, pandas; print('basic packages ok')"
```

如需检查 GPU / CUDA 相关包，可继续执行：

``` bash
python -c "import torch; print(torch.__version__); print(torch.cuda.is_available())"
```

### 3. 注意事项

- 不要用 `sudo conda install`。
- 不要把环境装到 root 目录下。
- 每个用户维护自己的 Conda 环境，避免互相污染。
- 如果环境创建失败，先不要手动乱改，保存报错信息后联系管理员。

## 9. 常用审计命令

当前登录用户：

``` bash
who
w
```

最近登录：

``` bash
last
lastlog
```

CPU 占用：

``` bash
ps -eo user,pid,ppid,cmd,%mem,%cpu --sort=-%cpu | head -n 30
```

内存占用：

``` bash
ps -eo user,pid,ppid,cmd,%mem,%cpu --sort=-%mem | head -n 30
```

磁盘占用：

``` bash
sudo du -h --max-depth=1 /home | sort -h
sudo du -h --max-depth=1 /data | sort -h
```
