# Insumo para o charting do `/wayfinder`

Levantado em 2026-10-07, antes de existir qualquer código. Isto **não** é o mapa: o mapa é a
issue com label `wayfinder:map` neste repo. Este arquivo é só o material bruto que entra no
charting, e fica obsoleto assim que o mapa existir.

## Destino (proposto)

Um "Orca de terminal": orquestrar vários agentes CLI em paralelo, cada um em seu git worktree,
com review de diff e navegação, tendo o Neovim como editor integrado em vez de um VS Code
embutido num Electron.

## Audiência

Pra mim agora, open source depois. Consequência de arquitetura que isso carrega: **o core não
pode ser Lua rodando dentro do nvim**. Precisa ser um processo/CLI próprio com o nvim como *um*
cliente, senão não há o que extrair depois.

## Fora de escopo

Companion mobile, relay na cloud, Design Mode por screenshot, integração Linear, worktrees por
SSH. Fechado no charting de propósito, pra o mapa não crescer pra sempre.

## Fatos do ambiente que mudam decisões

> Dois fatos desta tabela estavam errados e foram corrigidos em 2026-10-07 pelas pesquisas do mapa.
> O charting partiu deles, então a correção fica visível em vez de reescrita em silêncio.

| Fato | Por que importa |
| --- | --- |
| kitty com `allow_remote_control yes` mas `listen_on` no default `none`, tmux instalado, ghostty **não** instalado | **Fato corrigido.** A premissa de "duas superfícies possíveis" era falsa: sem `listen_on` o kitty não tem socket e não é dirigível, e o ghostty era só um symlink órfão de cask. Medido na issue #2: a multiplexação é tmux. |
| `sindrets/diffview.nvim` + `git-delta` + `lazygit` já em uso | O review pode ser extensão do que já existe em vez de camada nova. |
| nvim é LazyVim, config enxuta (6 plugins próprios) | Plugin novo entra fácil; não há arquitetura própria competindo. |
| Agentes instalados: `claude-code` e `opencode` | "Qualquer CLI agent" é promessa caríssima. O universo real hoje são dois. |
| fish como shell; rust e node disponíveis, mas `pnpm` **não roda** no node v22.5.1 instalado (exige v22.13+) | **Fato corrigido** na issue #3. Linguagem do core segue aberta, e escolher Node implica upgrade de node antes. fish importa porque não é POSIX: todo `shell-command` do tmux roda sob `/bin/sh`, nunca sob fish. |

## Os 10 focos de névoa

Cada um na forma que o wayfinder quer: uma **pergunta a decidir**, não código a escrever. O tipo
anotado é sugestão, não decisão.

1. **Quem é o dono do processo do agente?** (`grilling`) Roda num terminal-buffer do nvim, num
   painel do multiplexer, ou num daemon que sobrevive ao nvim fechar? Decisão-raiz: define se o
   projeto é plugin de nvim, TUI própria ou daemon + clientes finos. Nada abaixo decide antes dela.
2. **tmux, kitty remote control, ou multiplexer próprio?** (`research`) tmux dá detach e
   persistência e é universal (serve o "OSS depois"); kitty RC dá splits nativos mas amarra o
   projeto ao terminal. Levantar o que cada um permite programaticamente.
3. **Worktree por agente: quem cria, quem limpa, e como o worktree fica executável?** (`grilling`)
   Naming, onde moram, e o furo clássico: worktree novo não tem `.env`, `node_modules` nem
   `bundle`, então o agente abre num repo que não roda.
4. **Qual é a unidade que o usuário manipula?** (`grilling` + `domain-modeling`) No Orca é "o
   agente". Aqui é a task? o worktree? o prompt? Isso fixa o modelo de domínio e a UI inteira.
5. **Fan-out do mesmo prompt pra N agentes: uso real ou feature de demo?** (`grilling`) Custa N×
   tokens. Se fica, exige comparação lado a lado; se cai, o projeto encolhe pela metade.
6. **Review de diff: `diffview.nvim` resolve?** (`prototype`) A parte difícil não é ver o diff, é
   o comentário inline **voltar como prompt** pro agente que escreveu aquilo. Cabe sobre o
   diffview ou exige buffer próprio?
7. **Como o nvim conversa com o core?** (`research`) RPC nativo do nvim (`--listen`/`--server`),
   socket próprio, ou arquivo + watch? Isso também decide a linguagem do core.
8. **Onde o estado mora e a quê ele sobrevive?** (`grilling`) Arquivo no repo, SQLite em
   `~/.local/state`, memória do daemon? Sobrevive a reboot? É por repo ou global?
9. **Qual é o contrato mínimo de um agente CLI?** (`research`) Como se detecta que terminou, como
   se captura output sem perder o TUI dele, como se injeta prompt. Decidir com `claude-code` e
   `opencode` na mão antes de prometer os outros 40.
10. **O que do Orca fica explicitamente fora?** (`grilling`) Fechar no charting — é o que impede o
    mapa de crescer pra sempre. A lista em "Fora de escopo" acima é o ponto de partida.
