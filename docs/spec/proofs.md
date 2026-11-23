# Proofs

**Notation.** $`H`$ is the set of honest nodes; $`V_i(t)`$ is the view of node $`i`$ at time $`t`$; $`e`$ is the first view of the epoch under consideration; $`l = L(v)`$ is the honest leader of view $`v`$; $`a_i`$ is the time when $`i`$ first enters the epoch; $`D_i(u)`$ is the deadline for view $`u`$; $`d`$ is the upper bound on message delivery time after $`\mathrm{GST}`$; $`\tau`$ is the interval between consecutive deadlines.

**Assumptions.** The committee is fixed and consists of $`n=3f+1`$ nodes, at most $`f`$ of which are Byzantine. Leaders rotate in round-robin order. Each epoch has length $`n`$. Message delivery between nodes takes strictly positive time. Local processing is instantaneous. Schedules are consistent, and $`\tau\gt\mathrm{LT}+7d`$. The required blocks are available, and the blocksync delay is $`0`$. Votes cast by honest nodes are available only to their recipients. $`\mathrm{LT}`$ is the lock timer duration, with $`\mathrm{LT}\gt3d`$. A message sent by an honest node to an honest recipient at time $`s`$ is delivered no later than $`\max\{s,\mathrm{GST}\}+d`$. In `OnReceiveQC`, after certificate verification, the block certified by the QC and its required ancestors are synchronized and processed by `Update` in ancestor-to-descendant order before the view transition. This rule applies regardless of the source of the QC and the current view.

## Safety

**Definition (prefix).** A block $`b`$ is a prefix of a block $`b'`$, denoted by $`b\preceq b'`$, if there is a path from $`b'`$ to $`b`$ along parent links. A path of length zero is allowed, so $`b\preceq b`$.

**Definition (conflict).** Blocks $`b`$ and $`b'`$ conflict if

$$
b\npreceq b'\quad\land\quad b'\npreceq b.
$$

**Lemma S1 (uniqueness of the certified block in a view).** If valid certificates $`\mathrm{QC}_v(b)`$ and $`\mathrm{QC}_v(b')`$ exist, then $`b=b'`$.

**Proof.** Suppose that $`b\ne b'`$. Let $`Q`$ and $`Q'`$ be the sets of nodes whose votes are included in these certificates. Each certificate contains votes from at least $`2f+1`$ distinct nodes for the corresponding block in view $`v`$. Since there are $`n=3f+1`$ nodes in total, we obtain

$$
|Q\cap Q'|\ge |Q|+|Q'|-n\ge 2(2f+1)-(3f+1)=f+1.
$$

The intersection contains at least one honest node, since at most $`f`$ nodes are Byzantine. That node voted for two different blocks $`b`$ and $`b'`$ in view $`v`$, contradicting the protocol rule that an honest node votes at most once in each view. Therefore, $`b=b'`$. $`\square`$

**Lemma S2 (parent of a certified block).** If a valid certificate $`\mathrm{QC}_v(b)`$ exists for a block $`b\ne\mathrm{genesis}`$, then $`b.\mathrm{parent}=b.\mathrm{justify}.\mathrm{node}`$.

**Proof.** The certificate contains a vote from at least one honest node, which checked this equality before voting. $`\square`$

**Lemma S3 (monotonicity of voting).** If an honest node voted in view $`u`$, then every subsequent vote it casts is in a view $`w\gt u`$.

**Proof.** Before voting, the node checks `votedView < curView`, and when voting, it assigns `votedView ← curView`. $`\square`$

**Lemma S4 (prefix of certified blocks).** Suppose valid certificates $`\mathrm{QC}_v(b)`$ and $`\mathrm{QC}_{v+1}(b')`$ exist, with $`b'.\mathrm{justify}=\mathrm{QC}_v(b)`$. Then, for every valid certificate $`\mathrm{QC}_w(x)`$ with $`w\ge v`$,

$$
b\preceq x.
$$

**Proof.** Suppose there is a valid certificate $`\mathrm{QC}_w(x)`$ with $`w\ge v`$ and $`b\npreceq x`$. Choose such a certificate with the smallest $`w`$. By S1, if $`w=v`$, then $`x=b`$, and if $`w=v+1`$, then $`x=b'`$. By S2, block $`b'`$ is a child of $`b`$, so both cases are excluded and $`w\gt v+1`$.

Let $`x.\mathrm{justify}=\mathrm{QC}_q(p)`$. The quorums of certificates $`\mathrm{QC}_{v+1}(b')`$ and $`\mathrm{QC}_w(x)`$ intersect in at least $`f+1`$ nodes, so their common signers include an honest node $`i`$. By S3, it voted for $`b'`$ before voting for $`x`$.

When processing $`b'`$, node $`i`$ executes `Update` and raises its lock to at least $`v`$, since $`b'.\mathrm{justify}=\mathrm{QC}_v(b)`$. The lock view never decreases, so when voting for $`x`$, the lock check requires

$$
q\ge\mathrm{view}(\mathrm{lockQC}_i)\ge v.
$$

Before voting for $`x`$, node $`i`$ processes $`\mathrm{QC}_q(p)`$, so $`q\lt w`$. Otherwise, processing the certificate would move the node above $`w`$ and prevent it from voting in $`w`$.

If $`q\ge v`$, then the minimality of $`w`$ would imply $`b\preceq p`$. By S2, $`p=x.\mathrm{parent}`$, hence $`b\preceq p\preceq x`$, contradicting the choice of $`x`$. Therefore, $`q\lt v`$, contradicting the bound $`q\ge v`$ obtained above. $`\square`$

**Lemma S5 (chain of a certified block).** Suppose valid certificates $`\mathrm{QC}_v(b)`$ and $`\mathrm{QC}_{v+1}(b')`$ exist, with $`b'.\mathrm{justify}=\mathrm{QC}_v(b)`$. Then, for every valid certificate $`\mathrm{QC}_w(x)`$ with $`w\ge v+1`$, there is a child $`c`$ of block $`b`$ such that

$$
c\preceq x,\qquad c.\mathrm{justify}=\mathrm{QC}_v(b).
$$

**Proof.** Suppose there is a certificate $`\mathrm{QC}_w(x)`$ for which the claim is false, and choose such a certificate with the smallest $`w\ge v+1`$. If $`w=v+1`$, then by S1, $`x=b'`$, and by S2, block $`b'`$ is a child of $`b`$ with the required justify, so $`c=x=b'`$ satisfies the claim. Therefore, $`w\gt v+1`$.

Let $`x.\mathrm{justify}=\mathrm{QC}_q(p)`$. The quorums of certificates $`\mathrm{QC}_{v+1}(b')`$ and $`\mathrm{QC}_w(x)`$ intersect in at least $`f+1`$ nodes, so their common signers include an honest node $`i`$. By S3, it voted for $`b'`$ before voting for $`x`$.

When processing $`b'`$, node $`i`$ executes `Update` and raises its lock to at least $`v`$. The lock view never decreases, so the check before voting for $`x`$ requires $`q\ge v`$. Before voting, the node processes $`\mathrm{QC}_q(p)`$, so $`q\lt w`$: otherwise, it would move above $`w`$ and could not vote in $`w`$. Therefore,

$$
v\le q\lt w.
$$

- If $`q=v`$, then by S1, $`p=b`$, and by S2, $`x.\mathrm{parent}=p=b`$. Thus, $`c=x`$ satisfies the claim.
- If $`q\ge v+1`$, then by the minimality of $`w`$, there is a child $`c`$ of block $`b`$ with $`c\preceq p`$ and $`c.\mathrm{justify}=\mathrm{QC}_v(b)`$. By S2, block $`p`$ is the parent of $`x`$, so $`c\preceq p\preceq x`$.

Both cases contradict the choice of $`x`$. $`\square`$

**Theorem (safety).** If honest nodes have committed blocks $`b_1`$ and $`b_2`$, then

$$
b_1\preceq b_2\quad\lor\quad b_2\preceq b_1.
$$

**Proof.** If either block is genesis, the claim follows because genesis is a prefix of every committed block. Otherwise, for each $`j\in\{1,2\}`$, the commit of $`b_j`$ occurred while processing a block $`x_j`$ such that

$$
x_j.\mathrm{justify}=\mathrm{QC}_{v_j+1}(d_j),\qquad
d_j.\mathrm{justify}=\mathrm{QC}_{v_j}(c_j).
$$

Both certificates are valid, and $`d_j.\mathrm{parent}=c_j`$. During this processing, `Update` called $`\mathrm{onCommit}(c_j)`$, which committed $`c_j`$ and its ancestors that had not yet been committed, so $`b_j\preceq c_j`$.

Without loss of generality, $`v_1\le v_2`$. By S4, $`c_1\preceq c_2`$. Therefore, $`b_1\preceq c_2`$ and $`b_2\preceq c_2`$. Both blocks lie on the same chain of parent links from $`c_2`$, so one is a prefix of the other. $`\square`$

## Liveness

**Definition.** The deadline for view $`u`$ at node $`i`$ in the epoch under consideration is

$$
D_i(u):=a_i+(u-e+1)\tau.
$$

**Definition.** View $`v`$ is called a $`\mathrm{goodView}`$ if the first honest entry into its epoch occurred after $`\mathrm{GST}`$ and the leaders of $`v`$ and $`v+1`$ are honest.

**Definition.** View $`v`$ is called a $`\mathrm{commitView}`$ if $`v`$ is a $`\mathrm{goodView}`$ and the leader of $`v+2`$ is honest.

**Lemma L1.** Let $`E`$ be an epoch whose first honest entry occurred at time $`t\ge\mathrm{GST}`$. Then every honest node crosses the boundary of $`E`$ within the interval $`[t,t+2d]`$:

$$
\forall i\in H:\qquad t\le t_i(E)\le t+2d.
$$

Here, $`t_i(E)`$ is the time when node $`i`$ first reaches a view at least $`e`$.

**Proof.** Let $`j`$ be the first honest node to cross the boundary of $`E`$, at time $`t`$. It enters via $`\mathrm{TC}(e)`$ or $`\mathrm{QC}(e-1)`$. A certificate for a higher view requires an honest signer that had already crossed the boundary of $`E`$ before $`t`$, contradicting the choice of $`j`$.

When entering via $`\mathrm{QC}(e-1)`$, node $`j`$ has $`\mathrm{curView}\lt e`$ before the transition, so it broadcasts the QC to all nodes. When entering via $`\mathrm{TC}(e)`$, node $`j`$ broadcasts it to all nodes as a leader in the epoch. In either case, every honest node receives the certificate no later than $`t+d\le t+2d`$.

Processing the certificate moves the node to a view at least $`e`$ if it has not yet crossed that boundary. Therefore, $`t_i(E)\le t+2d`$. The choice of $`t`$ implies $`t_i(E)\ge t`$. $`\square`$

**Lemma L2 (view deadlines).** Suppose the first honest entry into epoch $`E`$ occurred after $`\mathrm{GST}`$, $`v\in E`$, and $`t_0`$ is the time of the first honest entry into $`v`$. Then, for any honest nodes $`i,j`$ that started the schedule for $`E`$,

$$
D_i(v)\ge t_0+\tau-2d,
\qquad
|D_i(v)-D_j(v)|\le2d.
$$

**Proof.** Let $`k`$ be the first honest node to enter $`v`$. It enters no later than $`a_k+(v-e)\tau`$. On its first entry into the epoch, this follows from $`t_0=a_k`$; for transitions within the epoch, the timers of preceding views bring it there. By Lemma L1, honest nodes' entry times into the epoch differ by at most $`2d`$, so

$$
D_i(v)-t_0\ge\tau+a_i-a_k\ge\tau-2d,
\qquad
|D_i(v)-D_j(v)|=|a_i-a_j|\le2d.\quad\square
$$

**Corollary to Lemma L2.** If $`\tau\gt\mathrm{LT}+7d`$, then for every honest node $`i`$ that started the schedule for $`E`$,

$$
D_i(v)\gt t_0+\mathrm{LT}+5d.
$$

**Lemma L3.** Suppose $`v\in E`$ and the first honest entry into epoch $`E`$ occurred after $`\mathrm{GST}`$. If the honest leader $`l = L(v)`$ enters $`v`$ at time $`t_0`$, then

$$
\forall i\in H:\quad V_i(t_0)\le v.
$$

**Proof.** Suppose there is an honest node $`i`$ with $`V_i(t_0) \gt v`$. Let $`t_1 \le t_0`$ be the time of the first transition by any honest node from a view $`\le v`$ to a view $`\gt v`$.

The leader enters $`v`$ no later than $`a_l+(v-e)\tau`$: either immediately upon entering the epoch or as a result of the timers of preceding views. A $`\mathrm{QC}`$ may accelerate its progress. By Lemma L1, for any honest node $`k`$ with a schedule for $`E`$, $`|a_k-a_l|\le2d`$. Therefore,

$$
t_0\le a_l+(v-e)\tau,\qquad
D_k(v)-t_0\ge\tau+a_k-a_l\ge\tau-2d \gt 0.
$$

Now consider the first transition above $`v`$ at $`t_1`$:

- Via a timeout of $`v`$ or $`\mathrm{TC}(v+1)`$. With a local timeout, an honest node $`k`$ transitions no earlier than $`D_k(v)`$. A $`\mathrm{TC}`$ requires a $`\mathrm{Wish}(v+1)`$ from an honest signer $`k`$, sent no earlier than $`D_k(v)`$; the $`\mathrm{TC}`$ can be assembled and used only after this signature. In either case, $`t_1 \ge D_k(v) \gt t_0`$.
- Via $`\mathrm{QC}(v)`$. The honest leader $`l`$ proposes only after entering $`v`$. The honest votes required for the $`\mathrm{QC}`$ and strictly positive message delivery time imply $`t_1 \gt t_0`$.
- Via $`\mathrm{QC}(q)`$, $`q \gt v`$, or $`\mathrm{TC}(q)`$, $`q \gt v+1`$. Here, $`q`$ is the certificate view. An honest signer was already above $`v`$ before $`t_1`$, contradicting the choice of the first transition.

In the first two cases, $`t_1 \gt t_0`$ contradicts $`t_1 \le t_0`$; the last case contradicts the minimality of $`t_1`$. Therefore, $`\forall i\in H:\ V_i(t_0)\le v`$. $`\square`$

**Corollary to Lemma L3.** Under the conditions of Lemma L3, when leader $`L(v)`$ enters $`v`$ at time $`s`$, for every honest node $`i`$ that started the schedule for $`E`$,

$$
D_i(v)\ge s+\tau-2d\gt s+\mathrm{LT}.
$$

**Lemma L4 (lock check).** Let $`v`$ be a $`\mathrm{goodView}`$, let $`b`$ be the block proposed by $`L(v)`$, and let $`t_i`$ be the time when honest node $`i`$ checks $`b`$ in view $`v`$. If $`\mathrm{QC}_v(b)`$ has not yet been assembled, then

$$
\mathrm{view}(\mathrm{lockQC}_i(t_i))
\le \mathrm{view}(b.\mathrm{justify}).
$$

Here, $`\mathrm{lockQC}_i(t_i)`$ is the lock of node $`i`$ before checking the proposal.

**Proof.** Let $`q=b.\mathrm{justify}`$. The honest leader proposes in $`v`$ with $`\mathrm{view}(q)\le v-1`$.

- If $`\mathrm{view}(q)=v-1`$, the node's lock in $`v`$ is no higher than $`q`$: a known $`\mathrm{QC}`$ with view at least $`v`$ would already have moved it above $`v`$.
- If $`\mathrm{view}(q)\lt v-1`$, no honest node has received $`\mathrm{QC}(v-1)`$ before the proposal: $`L(v)`$ is the first to assemble it. Honest nodes enter via timers or a $`\mathrm{TC}`$ and send status messages. With entry times differing by at most $`2d`$, the status messages arrive within $`3d\lt\mathrm{LT}`$; the chosen $`q`$ is at least as high as their $`\mathrm{highQC}`$ values and locks.

In either case, $`\mathrm{view}(\mathrm{lockQC}_i(t_i))\le\mathrm{view}(q)`$. $`\square`$

**Lemma L5 (QC assembly and delivery).** Let $`v`$ be a $`\mathrm{goodView}`$, let $`t_0`$ be the time of the first honest entry into $`v`$, and let $`b`$ be the block proposed by $`L(v)`$. Then the following bounds hold for $`\mathrm{QC}_v(b)`$:

If $`L(v)`$ entered $`v`$ via $`\mathrm{QC}(v-1)`$, then $`L(v+1)`$ assembles $`\mathrm{QC}_v(b)`$ no later than $`t_0+4d`$, and all honest nodes process it no later than $`t_0+5d`$.

If $`L(v)`$ entered $`v`$ via a timer or a $`\mathrm{TC}`$, then $`L(v+1)`$ assembles $`\mathrm{QC}_v(b)`$ no later than $`t_0+4d+\mathrm{LT}`$, and all honest nodes process it no later than $`t_0+5d+\mathrm{LT}`$.

**Proof.** Let $`t_1`$ be the time when $`L(v)`$ enters $`v`$, let $`t_p`$ be the time when $`b`$ is sent, and let $`q=b.\mathrm{justify}`$. The times $`t_1`$ and $`t_p`$ exist because, by assumption, the honest leader $`L(v)`$ proposed $`b`$ in view $`v`$.

If the first honest node entered via $`\mathrm{QC}(v-1)`$, the honest leader $`L(v)`$ was the first to assemble it. For entry via a timer, we use the bound of $`2d`$ on the difference between deadlines for $`v-1`$ from Lemma L2; for entry via a $`\mathrm{TC}`$ at an epoch boundary, we use Lemma L1. Therefore, $`t_0\le t_1\le t_0+2d`$.

- If $`L(v)`$ entered via $`\mathrm{QC}(v-1)`$, then $`t_p=t_1\le t_0+2d`$.
- If it entered via a timer or a $`\mathrm{TC}`$, then $`t_p\le t_1+\mathrm{LT}\le t_0+2d+\mathrm{LT}`$.

Therefore, every honest node $`i`$ receives $`b`$ at a time $`t_i\le t_p+d`$.

By the corollary to Lemma L2,

$$
D_i(v)\gt t_0+\mathrm{LT}+5d\ge t_p+3d\gt t_p+d.
$$

Thus, timeouts of $`v`$ do not occur before $`t_p+3d`$, which covers receipt of the proposal, assembly of the QC, and delivery of the next proposal.

If $`\mathrm{QC}_v(b)`$ has already been assembled by $`t_p+d`$, the required bound holds. Otherwise, by Lemma L3, all honest nodes are at or below $`v`$ when the leader enters; timeouts are excluded by the bound above, and the first honest transition above $`v`$ via a certificate requires $`\mathrm{QC}_v(b)`$. Therefore, all honest nodes remain at or below $`v`$ until $`t_p+d`$.

If a recipient is still below $`v`$ and $`\mathrm{view}(q)=v-1`$, processing $`q`$ moves it to $`v`$. If $`\mathrm{view}(q)\lt v-1`$, honest nodes enter via timers or a $`\mathrm{TC}`$ no later than $`t_0+2d`$, before the proposal sent after $`\mathrm{LT}`$.

Thus, every honest node checks $`b`$ in view $`v`$. By Lemma L4, its lock is no higher than $`q`$; since the honest leader $`L(v)`$ proposes a single block, the node votes for $`b`$ or has already done so. All honest votes are sent by $`t_p+d`$ and delivered to $`L(v+1)`$ by $`t_p+2d`$.

Therefore, in both cases, $`L(v+1)`$ assembles $`\mathrm{QC}_v(b)`$ no later than $`t_p+2d`$.

A transition above $`v`$ via a timeout or a $`\mathrm{TC}`$ is excluded by the bound $`D_i(v)\gt t_p+3d`$, and the first honest transition above $`v`$ via a $`\mathrm{QC}`$ requires $`\mathrm{QC}_v(b)`$, which $`L(v+1)`$ itself is the first to assemble. Therefore, the honest leader $`L(v+1)`$ could not have entered a view above $`v`$ before assembling $`\mathrm{QC}_v(b)`$. After assembling it, the leader enters $`v+1`$ and immediately broadcasts a proposal $`b^{\ast}`$ with $`b^{\ast}.\mathrm{justify}=\mathrm{QC}_v(b)`$. Every honest node receives it no later than $`t_p+3d`$.

The embedded $`\mathrm{QC}`$ is processed before the view of the proposal itself is checked: the recipient processes the certificate or already knows a certificate with view at least $`v`$. Thus, an early transition via a $`\mathrm{QC}`$ does not invalidate the bound. Substituting the two bounds on $`t_p`$ gives the deadlines stated in the lemma. $`\square`$

**Corollary to Lemma L5.** Let $`v-1`$ be a $`\mathrm{goodView}`$, and let $`b`$ be the block proposed by $`L(v-1)`$ in view $`v-1`$. Then $`L(v)`$ is the first honest node to enter $`v`$, at the time it assembles $`\mathrm{QC}_{v-1}(b)`$. Moreover, no honest node enters $`v`$ via a timeout of $`v-1`$.

**Lemma L6.** Let $`v`$ be a $`\mathrm{commitView}`$, let $`b`$ be the block proposed by leader $`L(v)`$ in view $`v`$, and let $`t_{\mathrm{commit}}^{\,i}(b)`$ be the time when honest node $`i`$ commits $`b`$. Then every honest node commits $`b`$, and

$$
\forall i\in H:\qquad
t_0\le t_{\mathrm{commit}}^{\,i}(b)\le t_{\mathrm{qc}} \lt \infty.
$$

Here:

- $`t_0`$ is the time of the first honest entry into $`v`$;
- $`t_{\mathrm{qc}}`$ is the time by which all honest nodes have received and processed the proposal from the leader of $`v+2`$.

If $`L(v)`$ entered $`v`$ via $`\mathrm{QC}(v-1)`$, then $`t_{\mathrm{qc}}\le t_0+9d`$.

If $`L(v)`$ entered $`v`$ via a timeout or a $`\mathrm{TC}`$, then $`t_{\mathrm{qc}}\le t_0+9d+\mathrm{LT}`$.

**Proof.** Let $`t_1`$ be the time when leader $`L(v+1)`$ assembles $`\mathrm{QC}_v(b)`$. By Lemma L5, $`t_1\le t_0+4d`$ if $`L(v)`$ entered via $`\mathrm{QC}(v-1)`$, and $`t_1\le t_0+4d+\mathrm{LT}`$ if it entered via a timeout or a $`\mathrm{TC}`$. The leader enters $`v+1`$ via this QC and immediately proposes a child block $`b^{\ast}`$ with $`b^{\ast}.\mathrm{justify}=\mathrm{QC}_v(b)`$.

View $`v+1`$ is also a $`\mathrm{goodView}`$. Let $`t_2`$ be the time when leader $`L(v+2)`$ assembles $`\mathrm{QC}_{v+1}(b^{\ast})`$. By the corollary to Lemma L5, the first honest entry into $`v+1`$ occurs at $`t_1`$, so applying Lemma L5 again for entry via a QC gives $`t_2\le t_1+4d`$.

At time $`t_2`$, leader $`L(v+2)`$ enters $`v+2`$ and immediately broadcasts a child block $`b^{\ast\ast}`$ with $`b^{\ast\ast}.\mathrm{justify}=\mathrm{QC}_{v+1}(b^{\ast})`$. Therefore,

$$
t_{\mathrm{qc}}\le t_2+d\le t_1+5d\lt\infty.
$$

By the corollary to Lemma L5, the first honest entry into $`v+2`$ occurs at $`t_2`$. By the corollary to Lemma L2, for every honest node $`i`$ with a schedule for the epoch containing $`v+2`$,

$$
t_2+d\lt t_2+\mathrm{LT}+5d\lt D_i(v+2).
$$

The deadlines of higher views in the same epoch are later still. A later epoch starts at an honest node no earlier than $`t_2`$, so its first active deadline is no earlier than $`t_2+\tau\gt t_2+\mathrm{LT}+5d`$. Thus, delivery of $`b^{\ast\ast}`$ to all honest nodes completes before any honest timeout of view $`v+2`$ or higher.

If an honest node is still at or below $`v+2`$ when it receives $`b^{\ast\ast}`$, the embedded QC moves it to $`v+2`$ if necessary. Processing $`b^{\ast\ast}`$ reveals the parent $`b^{\ast}`$ and two consecutive certificates for $`b`$ and $`b^{\ast}`$, so the node commits $`b`$.

If an honest node is already above $`v+2`$ at a time $`t\le t_2+d`$, the first honest transition above $`v+2`$ occurred via $`\mathrm{QC}_{v+2}(b^{\ast\ast})`$. Timeouts are excluded, and a higher QC requires a preceding honest transition above $`v+2`$.

The node itself moved above $`v+2`$ via some $`\mathrm{QC}_q(x)`$, $`q\ge v+2`$. By Lemma S5, the chain of block $`x`$ contains a child of $`b^{\ast}`$ with justify $`\mathrm{QC}_{v+1}(b^{\ast})`$. By the assumption on `OnReceiveQC`, this chain is processed by `Update` in ancestor-to-descendant order before the view transition, so the two consecutive certificates for $`b`$ and $`b^{\ast}`$ cause $`b`$ to be committed under the two-chain rule before the node moves above $`v+2`$.

In both cases, every honest node commits $`b`$ no later than $`t_{\mathrm{qc}}`$. Substituting the bounds on $`t_1`$ into $`t_{\mathrm{qc}}\le t_1+5d`$ yields $`t_{\mathrm{qc}}\le t_0+9d`$ if $`L(v)`$ entered via $`\mathrm{QC}(v-1)`$, and $`t_{\mathrm{qc}}\le t_0+9d+\mathrm{LT}`$ if it entered via a timeout or a $`\mathrm{TC}`$. $`\square`$

**Lemma L7 (existence of a commitView).** Every epoch $`E`$ whose first honest entry occurred after $`\mathrm{GST}`$ contains a $`v\in E`$ that is a $`\mathrm{commitView}`$.

**Proof.** Consider the first $`n`$ views of epoch $`E`$ and, for each of them, the triple $`L(v),L(v+1),L(v+2)`$. This gives $`n`$ triples in total. The final triples may extend into the next epoch, as allowed by the definition of $`\mathrm{commitView}`$.

Under round-robin rotation, each Byzantine node appears in at most $`3`$ such triples, once in each position. Therefore, at most $`3f`$ triples contain at least one Byzantine leader, and the number of triples with three honest leaders is at least

$$
n-3f=(3f+1)-3f=1.
$$

Therefore, there is a $`v\in E`$ such that $`L(v),L(v+1),L(v+2)\in H`$. Since the first honest entry into $`E`$ occurred after $`\mathrm{GST}`$, this $`v`$ is a $`\mathrm{commitView}`$. $`\square`$

**Lemma L8 (entry of an honest leader into a view).** Suppose $`v\in E`$, the first honest entry into epoch $`E`$ occurred after $`\mathrm{GST}`$, and $`l=L(v)\in H`$. If, at time $`t^{\ast}`$, there is an honest node $`i`$ with $`V_i(t^{\ast})\gt v`$, then

$$
\exists t\lt t^{\ast}:\qquad V_l(t)=v.
$$

**Proof.** Suppose leader $`l`$ never entered $`v`$ before $`t^{\ast}`$. Let $`t'\le t^{\ast}`$ be the time of the first honest transition above $`v`$, and let $`v'=V_j(t')\gt v`$ be the view entered by honest node $`j`$.

We show that $`v'=v+1`$.

1. For a transition via $`\mathrm{QC}(v'-1)`$, an honest signer was in $`v'-1`$ before $`t'`$. Therefore, $`v'-1\le v`$.
2. For a transition via a timeout of $`v'-1`$, node $`j`$ itself was in $`v'-1`$ before $`t'`$. For $`\mathrm{TC}(v')`$, an honest sender of $`\mathrm{Wish}(v')`$ was in $`v'-1`$. Again, $`v'-1\le v`$.

Therefore, $`v'=v+1`$. Consider the transition into this view.

1. Via $`\mathrm{QC}(v)`$. The honest leader $`l`$ proposed a block after entering $`v`$, and the certificate was assembled later. Thus, the leader entered $`v`$ before $`t'\le t^{\ast}`$. A contradiction.
2. Via a timeout of $`v`$ or $`\mathrm{TC}(v+1)`$. Both cases require an honest timeout of $`v`$ at some node $`k`$, so $`t'\ge D_k(v)`$. By Lemma L1, the leader crosses the boundary of $`E`$ no later than $`a_k+2d\lt D_k(v)\le t'`$. Before $`t'`$, honest nodes are at or below $`v`$, so the leader starts the schedule for $`E`$, and the timers bring it to $`v`$ no later than $`a_l+(v-e)\tau`$. Moreover,

$$
a_l+(v-e)\tau
=D_k(v)-\tau+(a_l-a_k)
\le D_k(v)-\tau+2d
\lt D_k(v)\le t'.
$$

Therefore, the leader entered $`v`$ before $`t'\le t^{\ast}`$. A contradiction. $`\square`$

**Lemma L9 (exit from any epoch).** For every epoch, there is a finite time by which all honest nodes are above that epoch.

$$
\forall E\;\exists T_E\ge\mathrm{GST}\;\forall i\in H:
\qquad V_i(T_E)\gt r_E.
$$

Here, $`r_E`$ is the last view of epoch $`E`$.

**Proof.** Let $`t_0=\mathrm{GST}`$, $`v_{\max}=\max_{i\in H}V_i(t_0)`$, let $`E_{\max}`$ be the epoch containing $`v_{\max}`$, and let $`e_{\max}`$ be its first view.

If some honest nodes are below $`e_{\max}`$ at time $`t_0`$, consider the very first crossing of this boundary by any honest node. As in the proof of Lemma L1, it occurred via $`\mathrm{TC}(e_{\max})`$ or $`\mathrm{QC}(e_{\max}-1)`$. The first honest recipient broadcast the certificate to all nodes no later than $`t_0=\mathrm{GST}`$, and all honest nodes receive it no later than $`t_0+d`$. A recipient may cross the boundary via a higher QC before this certificate is delivered. Therefore,

$$
\forall i\in H:\qquad V_i(t_0+d)\ge e_{\max}.
$$

Let $`e_2`$ be the first view of epoch $`E_2=E_{\max}+1`$, and let $`r=e_2-1`$. Suppose an honest node $`k`$ never leaves $`E_{\max}`$. If any honest node crosses the boundary $`e_2`$, the first such transition occurs after $`t_0`$. By Lemma L1, all honest nodes, including $`k`$, cross it within the next $`2d`$. Thus, our assumption implies that no honest node crosses $`e_2`$.

In particular, under this assumption, an honest transition via a QC to a view at least $`e_2`$ is impossible. QCs may advance nodes within $`E_{\max}`$, and the deadlines are determined by that epoch's schedule.

In each view below $`r`$, a node either advances earlier via a QC or transitions via a timer. There are finitely many views before $`r`$, so every honest node reaches $`r`$ and sends $`\mathrm{Wish}(e_2)`$ by its deadline $`D_i(r)`$.

There is an honest leader among the leaders of $`E_2`$. In finite time, it receives at least $`2f+1`$ honest Wish messages, including those sent before $`\mathrm{GST}`$, assembles $`\mathrm{TC}(e_2)`$, and enters $`e_2`$. This contradicts the conclusion above that no honest node crosses $`e_2`$. Hence, all honest nodes leave $`E_{\max}`$ in finite time.

We show that some honest node visits $`e_2`$ itself. Let $`v'`$ be the smallest view reached by honest nodes that is at least $`e_2`$. If $`v'\gt e_2`$, then for entry via $`\mathrm{QC}(v'-1)`$ or $`\mathrm{TC}(v')`$, an honest signer was in $`v'-1`$; for entry via a local timeout, the transitioning node itself was in that view. In every case, a view $`e_2\le v'-1\lt v'`$ was reached, contradicting the minimality of $`v'`$. Therefore, $`v'=e_2`$.

The first honest crossing of the boundary of $`E_2`$ occurs after $`\mathrm{GST}`$, and by Lemma L1, all honest nodes reach a view at least $`e_2`$ within the next $`2d`$. Repeating the exit argument for $`E_2,E_2+1,\ldots`$ establishes the result for any subsequent epoch in finitely many steps. All honest nodes have already left epochs below $`E_{\max}`$ by $`t_0+d`$. $`\square`$

**Theorem (liveness).**

$$
\forall t\ge\mathrm{GST}\;\exists T\gt t:\qquad
\min_{i\in H}h_i(T)\gt\max_{i\in H}h_i(t).
$$

Here, $`h_i(t)`$ is the height of the last block committed by honest node $`i`$ by time $`t`$.

**Proof.** Let $`t\ge\mathrm{GST}`$. Choose an epoch $`E`$ whose first view $`e`$ satisfies

$$
e\gt\max_{i\in H}V_i(t).
$$

By Lemma L9, honest nodes reach and leave it in finite time, and the first entry occurs after $`t`$. By Lemma L7, there is a $`v\in E`$ that is a $`\mathrm{commitView}`$. By Lemma L9, some honest node eventually moves above $`v`$, so by Lemma L8, leader $`L(v)`$ enters $`v`$ at a time $`s\gt t`$.

By Lemma L3 and its corollary, timeouts of $`v`$ occur after $`s+\mathrm{LT}`$, and the first honest transition above $`v`$ via a QC requires a proposal from $`L(v)`$. Therefore, the leader proposes a block $`b`$ no later than $`s+\mathrm{LT}`$.

By Lemma L6, all honest nodes commit $`b`$ after $`t`$ and no later than some finite time $`T\gt t`$. A new commit increases a node's committed height, so $`\mathrm{height}(b)\gt h_i(t)`$ for every $`i\in H`$. Therefore,

$$
\min_{i\in H}h_i(T)
\ge\mathrm{height}(b)
\gt\max_{i\in H}h_i(t).
$$

Since $`t`$ was arbitrary, the proof is complete. $`\square`$
