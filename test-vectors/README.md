# Shared test vectors

`vectors.json` **不是本仓原创**：它逐字节取自
`ZT-DSP-Suite/ZT-DSP-client` 仓的 `test-vectors/vectors.json`，
即规范附录 J 的一致性向量，由文档仓统一维护。

服务端复用它，而不是各写一份 —— 一致性向量一旦分叉就没有意义。

## 服务端怎么用它

服务端**不解密、不派生密钥**，所以这 9 个向量里绝大多数对它没有直接约束。
但派生标签的命名规则是服务端必须与规范保持一致的间接契约：
客户端与服务端对 `-v3` 后缀的理解一旦分叉，双方就永远通不了。

`src/main.rs` 里的 `shared_vectors_use_the_v3_suffix` 就是守这条的。

## 已知上游不一致

`vectors.json` 里 `INFO_EK_ROTATE` 的值是 `ZT-DSP-EK-ROTATE`，
是 7 个标签里唯一**不带** `-v3` 后缀的。

这是本仓在复用时观察到的**上游数据不一致**，不是本实现的选择。
本仓不擅自改上游数据，已在 PR 中向文档仓 owner 提问；
在对方确认前，测试里把它列为显式例外。
