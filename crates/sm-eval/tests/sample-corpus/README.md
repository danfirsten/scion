# Sample corpus

**Generated — do not edit by hand.** Reproduce with:

```
sm-eval sample --corpus corpus --out crates/sm-eval/tests/sample-corpus
```

Fifteen real three-way merge conflicts mined from public Java history
(SPEC §6.1, milestone M1). The full corpus is gitignored; this slice is
committed so that later milestones have real Java to work against
without re-mining gigabytes of history. `crates/sm-eval/tests/sample_corpus.rs`
asserts its shape.

Each directory holds `base.java`, `ours.java`, `theirs.java`,
`resolved.java` (the human's committed resolution) and a `case.json`
recording the repository, the merge commit, both parents, the merge
base and the licence. `index.json` is every `case.json` in one file.

## Provenance

These files are unmodified excerpts of the upstream projects, taken at
the commits named below, and are redistributed under each project's own
licence. Only permissively licensed projects are sampled.

| case | project | licence | merge commit | path |
|---|---|---|---|---|
| `0872090fa4fb-dubbo-rpc_dubbo-rpc-api_src_main_java_org_apache_dubbo_rpc_model_ApplicationModel.java` | [apache/dubbo](https://github.com/apache/dubbo) | Apache-2.0 | `0872090fa4fb` | `dubbo-rpc/dubbo-rpc-api/src/main/java/org/apache/dubbo/rpc/model/ApplicationModel.java` |
| `27ad9858d215-maven-aether-provider_src_main_java_org_apache_maven_repository_internal_LocalSnapshotMetadataGenerator.java` | [apache/maven](https://github.com/apache/maven) | Apache-2.0 | `27ad9858d215` | `maven-aether-provider/src/main/java/org/apache/maven/repository/internal/LocalSnapshotMetadataGenerator.java` |
| `2947d147b47e-samples_server_petstore_springboot-spring-pageable-without-j8_src_main_java_org_openapitools_api_PetApi.java` | [OpenAPITools/openapi-generator](https://github.com/OpenAPITools/openapi-generator) | Apache-2.0 | `2947d147b47e` | `samples/server/petstore/springboot-spring-pageable-without-j8/src/main/java/org/openapitools/api/PetApi.java` |
| `2f113ca1024b-rocketmq-tools_src_main_java_com_alibaba_rocketmq_tools_admin_MQAdminExt.java` | [apache/rocketmq](https://github.com/apache/rocketmq) | Apache-2.0 | `2f113ca1024b` | `rocketmq-tools/src/main/java/com/alibaba/rocketmq/tools/admin/MQAdminExt.java` |
| `53d5003630ce-http-server-netty_src_main_java_io_micronaut_http_server_netty_NettyHttpServer.java` | [micronaut-projects/micronaut-core](https://github.com/micronaut-projects/micronaut-core) | Apache-2.0 | `53d5003630ce` | `http-server-netty/src/main/java/io/micronaut/http/server/netty/NettyHttpServer.java` |
| `57057f47496d-pmd-core_src_main_java_net_sourceforge_pmd_PMD.java` | [pmd/pmd](https://github.com/pmd/pmd) | BSD-3-Clause | `57057f47496d` | `pmd-core/src/main/java/net/sourceforge/pmd/PMD.java` |
| `61e597159433-sopremo_sopremo-common_src_main_java_eu_stratosphere_sopremo_testing_SopremoTestPlan.java` | [apache/flink](https://github.com/apache/flink) | Apache-2.0 | `61e597159433` | `sopremo/sopremo-common/src/main/java/eu/stratosphere/sopremo/testing/SopremoTestPlan.java` |
| `6bfef6c19165-samples_server_petstore_springboot-beanvalidation-no-nullable_src_main_java_org_openapitools_api_AnotherFakeApi.java` | [OpenAPITools/openapi-generator](https://github.com/OpenAPITools/openapi-generator) | Apache-2.0 | `6bfef6c19165` | `samples/server/petstore/springboot-beanvalidation-no-nullable/src/main/java/org/openapitools/api/AnotherFakeApi.java` |
| `823958bcc58c-spring-web_src_test_java_org_springframework_http_converter_protobuf_ProtobufHttpMessageConverterTests.java` | [spring-projects/spring-framework](https://github.com/spring-projects/spring-framework) | Apache-2.0 | `823958bcc58c` | `spring-web/src/test/java/org/springframework/http/converter/protobuf/ProtobufHttpMessageConverterTests.java` |
| `9f110e9099c0-pmd-java_src_main_java_net_sourceforge_pmd_lang_java_rule_design_LoosePackageCouplingRule.java` | [pmd/pmd](https://github.com/pmd/pmd) | BSD-3-Clause | `9f110e9099c0` | `pmd-java/src/main/java/net/sourceforge/pmd/lang/java/rule/design/LoosePackageCouplingRule.java` |
| `a37c84b7c5d3-_java_org_springframework_boot_test_autoconfigure_web_servlet_MockMvcWebDriverAutoConfiguration.java-c4343ca6037c5deb` | [spring-projects/spring-boot](https://github.com/spring-projects/spring-boot) | Apache-2.0 | `a37c84b7c5d3` | `spring-boot-project/spring-boot-test-autoconfigure/src/main/java/org/springframework/boot/test/autoconfigure/web/servlet/MockMvcWebDriverAutoConfiguration.java` |
| `a37c84b7c5d3-g-boot-tools_spring-boot-loader-tools_src_main_java_org_springframework_boot_loader_tools_Layer.java-b10116fcf54aa1d3` | [spring-projects/spring-boot](https://github.com/spring-projects/spring-boot) | Apache-2.0 | `a37c84b7c5d3` | `spring-boot-project/spring-boot-tools/spring-boot-loader-tools/src/main/java/org/springframework/boot/loader/tools/Layer.java` |
| `bfdad831842f-junit-engine-api_src_main_java_org_junit_gen5_engine_TestDescriptor.java` | [junit-team/junit-framework](https://github.com/junit-team/junit-framework) | EPL-2.0 | `bfdad831842f` | `junit-engine-api/src/main/java/org/junit/gen5/engine/TestDescriptor.java` |
| `e41ffbc55565-vertx-core_src_test_java_io_vertx_test_core_ClusteredEventBusTest.java` | [eclipse-vertx/vert.x](https://github.com/eclipse-vertx/vert.x) | EPL-2.0 | `e41ffbc55565` | `vertx-core/src/test/java/io/vertx/test/core/ClusteredEventBusTest.java` |
| `e6f229da6321-core_src_test_java_com_netflix_conductor_core_execution_TestConfiguration.java` | [Netflix/conductor](https://github.com/Netflix/conductor) | Apache-2.0 | `e6f229da6321` | `core/src/test/java/com/netflix/conductor/core/execution/TestConfiguration.java` |
