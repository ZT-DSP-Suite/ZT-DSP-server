# Shared test vectors

`vectors.json` **不是本仓原创**：它逐字节取自
`ZT-DSP-Suite/ZT-DSP-client` 仓的 `test-vectors/vectors.json`，
即规范附录 J 的一致性向量，由文档仓统一维护。

服务端复用它，而不是各写一份 —— 一致性向量一旦分叉就没有意义。

## 这里有什么

| 文件 | 内容 |
|---|---|
| `vectors.json` | 共享一致性向量，逐字节取自 `ZT-DSP-client` 仓 |

## 按什么找

- 消费点：`src/version.rs` 的 `labels_match_the_spec_verbatim`。
- 源文件：`ZT-DSP-client/test-vectors/vectors.json`。

## 服务端怎么用它

服务端**不解密、不派生密钥**，所以这 9 个向量里绝大多数对它没有直接约束。
但派生标签本身是服务端必须与规范保持一致的间接契约：
客户端与服务端对标签串的理解一旦分叉，双方就永远通不了。

`src/version.rs` 里的 `labels_match_the_spec_verbatim` 就是守这条的。
它把 7 个标签的期望值逐字写死再比对，**不是**套「都该带 `-v3` 后缀」
这种规则：规范里 `INFO_EK_ROTATE` 对应的串是 `ZT-DSP-EK-ROTATE`，
本就不带后缀，套规则会把它误判成漏了。
