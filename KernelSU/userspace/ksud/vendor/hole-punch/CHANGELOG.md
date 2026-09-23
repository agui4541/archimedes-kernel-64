<a name="0.0.3"></a>
## 0.0.3 (2020-03-12)


#### Other Changes

*   Tests were creating sparse files incorrectly ([7192c4df](7192c4df))

#### Bug Fixes

*   Swtich from unix to whitelist of operating systems ([df90189b](df90189b))
*   Change error handling to use errno crate ([5d41017c](5d41017c))



<a name="0.0.2"></a>
## 0.0.2 (2020-03-09)


#### Features

*   Basic Windows Support ([03720f45](03720f45))
*   Add short circut on non-sparse files for windows ([733740e1](733740e1))
*   Create boiler plate windows module ([01a40e65](01a40e65))
*   Add winapi dependecny on windows ([36b1948f](36b1948f))
*   Add utility functions to Segment type ([4a2a50fe](4a2a50fe))

#### Other Changes

*   Set test files as sparse on windows ([bc905ddd](bc905ddd))
*   Add quickcheck for "holes have no data" ([ceb63e6d](ceb63e6d))



<a name="0.0.1"></a>
## 0.0.1 (2020-03-05)


#### Bug Fixes

*   Fix bugs that cropped up with quicktest ([48925620](48925620))
*   Unix Backend no longer ingores data at the end ([f5de2f8b](f5de2f8b))
*   Add extra saftey to unix SparseFile impl ([e13303cb](e13303cb))

#### Other Changes

*   Implement quickcheck for covers all bytes property ([e27b4f13](e27b4f13))
*   Implement framework for using quickcheck ([9186bea2](9186bea2))
*   Rewrite unix implementation ([ec618837](ec618837))
*   Add testing dependencies ([abe3bc4d](abe3bc4d))

#### Features

*   Add UnsupportedFileSystem to error type ([63346df6](63346df6))
*   add hole_info binary ([9551cf1e](9551cf1e))
*   Add debug derives to structs ([1613a33c](1613a33c))
*   Implement SparseFile for Unix ([e1b323ef](e1b323ef))
*   Add Unix Module ([761c30c7](761c30c7))
*   Add derives to types ([9c58b9a7](9c58b9a7))
*   Add libc depedency on Unix ([622ba069](622ba069))
*   Add default implementation ([aa9b9e49](aa9b9e49))
*   Add cfg-if dependency ([5f145c86](5f145c86))
*   Add SparseFile Trait ([b1d8e6d0](b1d8e6d0))



